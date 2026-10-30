# Rust performance evidence

[Documentation](../docs/README.md) · [Performance overview](../docs/performance.md) · [Features](../docs/features.md)

This is the dated measurement record. For a guided comparison, start with the [performance overview](../docs/performance.md). Each section below describes the source, version and fixture used at that time.

| Find a result | Recorded evidence |
| --- | --- |
| A focused VRR game next to another animating window: WGC dirty regions tell the game's pictures from the other window's updates and pointer moves; repeats, age, present-to-sent, with and without the pointer moving (ABBA) | [October 30, a focused game's pictures beside another window](#october-30-a-focused-games-pictures-beside-another-window) |
| Nightjar desktop presenting for VRR: the display check on a 240 Hz FreeSync panel and a fixed 120 Hz laptop panel, what it costs, mid-scan presents, sync interval 1 and waiting out the scan (not kept as defaults) | [October 29, Nightjar desktop on a VRR panel and a fixed one](#october-29-nightjar-desktop-on-a-vrr-panel-and-a-fixed-one) |
| False slow-pace latches from #59's rc.4 logs: a decoder asking twice for a keyframe, a host adapter refusing packets (lifted after 10 s), holding frames for a full socket (not kept), the keyframe cap scale (inert here); latches, stalls, picture age, presentation gaps (ABBA) | [October 29, false slow-pace latches and a stalled adapter](#october-29-false-slow-pace-latches-and-a-stalled-adapter-rc4-field-logs) |
| The console's Stream page against the browser page: glass-to-glass split at the frame's arrival, page part against the network and host part | [October 28, the console's Stream page against the browser page](#october-28-the-consoles-stream-page-against-the-browser-page-d4) |
| Scene cuts AMF encodes past the full-FEC bound, encoded again as keyframes: lost cuts, freezes, keyframe requests, end-to-end picture age, encode time (ABBA) | [October 28, oversize scene cuts encoded again as keyframes](#october-28-oversize-scene-cuts-encoded-again-as-keyframes) |
| LTR recovery on H.264 and HEVC with header rewrites: repair against keyframe bytes per gap (1 to 130) and codec, scrolling and static, decodes, the size estimate's choices, rewrite cost in a 60 s stream (ABBA) | [October 28, LTR recovery with header rewrites](#october-28-ltr-recovery-on-h264-and-hevc-with-header-rewrites-on-by-default) |
| Scene cuts on a clean route with 0.3% random loss: the automatic frame cap held within full four-block FEC (fixed, automatic and no FEC), oversize-only pacing (not kept), freezes, recovery keyframes, end-to-end picture age | [October 28, lost scene cuts and recovery keyframes past four FEC blocks](#october-28-lost-scene-cuts-and-recovery-keyframes-past-four-fec-blocks) |
| Audio passthrough: AC3 and E-AC3 encode time per packet against Opus, codec delay, and what the 32 ms frames cost end to end | [October 27, audio passthrough encode cost](#october-27-audio-passthrough-encode-cost) |
| AV1 intra refresh on AMF: keyframes, frame sizes and recovery after a lost frame, against LTR invalidation and keyframes (refresh turned off) | [October 27, AV1 intra refresh and recovery after a lost frame](#october-27-av1-intra-refresh-and-recovery-after-a-lost-frame) |
| HDR10+ and HDR Vivid dynamic metadata: the units in real AMF HEVC and AV1 output, what a Moonlight client receives, CPU cost per frame, 4K60 encode stage on and off (ABBA) | [October 27, HDR10+ and HDR Vivid dynamic metadata](#october-27-hdr10-and-hdr-vivid-dynamic-metadata) |
| A 60 Hz game's repeated frames under VRR: holding later compositions of a claimed frame, simulation and 60/120/240 Hz ABBA (not kept) | [October 26, holding later compositions (not kept)](#october-26-holding-later-compositions-of-a-claimed-frame-not-kept) |
| Replying from the address the client reached (#54): send-path cost of matched, routed and pinned replies against the calls before the fix, batch and single datagram | [October 26, replying from the address the client reached](#october-26-replying-from-the-address-the-client-reached-54) |
| VRR claims that follow the game's pictures: a 120 Hz game on a 60 fps VRR stream, age and present-to-sent p95, late claims; 240 and 60 Hz games | [October 26, VRR claims follow the game's pictures](#october-26-vrr-claims-follow-the-games-pictures) |
| 3.0.0-rc.2 against the published numbers: timing record stages, VRR capture age, the laptop over Wi-Fi; what could not be rerun | [October 26, 3.0.0-rc.2 against the published numbers](#october-26-300-rc2-against-the-published-numbers) |
| Wi-Fi clients paced from the first frame (`clientWireless`): Nightjar desktop over 5 GHz and 2.4 GHz, with and without the hint | [October 25, Wi-Fi pacing hint](#october-25-wi-fi-pacing-hint-on-a-real-laptop) |
| WebRTC in a browser on the real host: send pacing on and off, RTX at 0.1% loss, 90 and 120 fps at 1080p, 1440p and 4K, 4K keyframe-heavy, 4K60 HDR from the virtual display | [October 25, WebRTC on the real host (B5b)](#october-25-webrtc-on-the-real-host-pacing-rtx-90-and-120-fps-4k-and-hdr-b5b) |
| Behaviour across client link speeds: 1000 to 10 Mbit/s, step changes, Wi-Fi profile, what breaks above the link | [October 23, behaviour across client link speeds](#october-23-behaviour-across-client-link-speeds) |
| Near-link tuning of the link-rate controller: pace above the delivered rate, headroom 0.80, tight AMF buffer (all reverted) | [October 24, near-link tuning (L6)](#october-24-near-link-tuning-three-steps-tried-and-reverted-l6) |
| Nightjar desktop PyroWave decode: stage split, single upload (kept), batched passes (reverted), the present clock holding every frame 50 ms at the start | [October 23, PyroWave desktop decode stage split (brief 1)](#october-23-pyrowave-desktop-decode-stage-split-brief-1) |
| PyroWave at 144 fps: normal against high process priority (reverted), async encode (not worth it), handles left by each encoder or decoder life | [October 23, PyroWave 144 fps pacing and async encode (B4)](#october-23-pyrowave-144-fps-pacing-and-async-encode-b4) |
| Nightjar Android, Rust engine against the C engine on a phone: picture age, decode, 1% and 5% loss, PyroWave, 30-minute soak | [October 23, Rust engine against the C engine on Android (B8)](#october-23-rust-engine-against-the-c-engine-on-android-b8) |
| 4:2:0 against packed 4:4:4 on text and UI (quality, encode and decode time), and whether AFMF frames reach WGC capture, measured on the 780M | [October 23, 4:2:0 against packed 4:4:4 and AFMF capture (B3, B7), 780M](#october-23-420-against-packed-444-on-text-and-ui-and-afmf-capture-b3-b7-measured-on-the-780m) |
| Stream presets (`stream_preset`): frame cap applied, keyframe size, end-to-end latency | [October 23, stream presets (B3)](#october-23-stream-presets-b3) |
| Split-frame encoding through AMD's own sample encoder | [October 22, AMD's own sample encoder](#october-22-amds-own-sample-encoder) |
| Slice delivery (`video_slice_delivery = 4`) against off: loopback, paced Wi-Fi emulation, real Wi-Fi (laptop), 1% loss, SSIM | [October 23, slice delivery (B2 step 2f)](#october-23-slice-delivery-b2-step-2f) |
| Per-frame stage times from the timing record, and what the record costs | [October 23, timing record baselines](#october-23-per-frame-timing-record-baselines) |
| 3.0 alpha.2 gate: couch loop, crash restore, cross-versions, soak | [October 22, alpha.2 gate](#october-22-30-alpha2-gate) |
| Fence ring against keyed-mutex ring under GPU load | [October 22, fence ring under load](#october-22-fence-ring-under-gpu-load) |
| 3.0 alpha.1 hardware gate against 2.6.0 | [October 22, alpha.1 gate](#october-22-30-alpha1-hardware-gate) |
| Timer CPU cost and idle capture waits | [October 18, timer waits and idle capture](#october-18-timer-waits-and-idle-capture) |
| Retired compute queues and conservative GPU priority | [October 18, GPU validation](PERFORMANCE_WORK.md#october-18-retired-compute-queues-and-conservative-gpu-priority) |
| Repeated frame loss and Windows send-buffer pressure | [October 17, loss-response verification](#october-17-loss-response-verification) |
| Diagnosing repeated pictures despite full encoded FPS | [October 17, fresh-picture throughput diagnosis](#october-17-fresh-picture-throughput-diagnosis) |
| Installed-driver fence decision for 2.5.1 | [October 17, installed-driver fence comparison](#october-17-installed-driver-fence-comparison) |
| 2.5.0 recovery, scaling and optional fence transport | [October 17, focused 2.5.0 comparisons](#october-17-focused-250-comparisons) |
| Punktfunk host-timing parity and paced keyframes | [October 17, separate packetization and sending](#october-17-punktfunk-host-timing-and-paced-keyframes) |
| Where host latency goes | [October 15, present to last packet sent by stage, beside Moonlight's field](#october-15-where-host-latency-goes) |
| Frame push from Rubylight's display driver | [October 15, idd-push against WGC on the same virtual display](#october-15-frame-push-from-rubylights-display-driver) |
| Display driver and frame push refactor | [October 15, where the frame push spends its time after the refactor](#october-15-display-driver-and-frame-push-refactor) |
| Frame rates above 60 | [October 14, 1440p at 60-240 fps and 20-500 Mbps](#october-14-frame-rates-above-60) |
| A port slower than the host | [October 14, keyframe loss through a 100 Mbps bottleneck](#october-14-keyframe-loss-on-a-slow-port) |
| HDR and recovery fixes | [October 14, 2.2.17 against 2.2.18](#october-14-hdr-and-recovery-fixes-2218) |
| Rubylight 2.2.9 refresh | [October 12 released-package encoder probes](#october-12-rubylight-229-released-package-refresh) |
| Radeon compute off/on | [1080p60 comparison](#1080p-at-60-fps) |
| D3D11 4:2:0 fallback | [October 12 single-pass conversion](#october-12-single-pass-d3d11-420-conversion) |
| Real laptop client over Wi-Fi | [2.2.9 current receiver](PERFORMANCE_WORK.md#real-wi-fi-hardware-receiver-streams-october-12) · [Render to received, rc.24](PERFORMANCE_WORK.md#october-8-real-client-picture-age-over-wi-fi-rc24-laptop) |
| Other Sunshine hosts | [Vibepollo 2.0 baseline](#against-vibepollo-20) · [rc.24 on the same GPU](PERFORMANCE_WORK.md#rc24-against-vibepollo-20-on-the-same-gpu) |
| WGC capture and pacing | [Guarded comparisons](#guarded-ab-stress-and-compatibility-checks) |
| Native HEVC / AV1 HDR | [2.2.9 headline streams](#native-virtual-hdr-results-october-12) · [Final rc.10 pixels and repeat runs](#final-native-virtual-hdr-pixels-excluding-physical-panel-calibration) |
| PyroWave HDR 4:4:4 | [Transport and decoding](#vibepollo-20-pyrowave-transport) · [SDK patches from Punktfunk](#october-15-pyrowave-sdk-patches-from-punktfunk) |
| CPU error correction | [C++ FEC comparison](#controlled-comparison-with-the-original-c-fec) |
| Reproduction and scope | [Probes](#reproduce-on-another-machine) · [Hardware covered](#hardware-covered) |

## October 30: a focused game's pictures beside another window

A 60 Hz game streamed at 60 fps under VRR from the 240 Hz primary display (5120x1440, display capture through WGC) repeated and skipped frames while another window on the same display animated: every composition counted as a picture of its own, so claims drifted through the game's frames. With this change, on Windows 11 24H2 and newer, the capture reads each composition's dirty regions and the foreground window's bounds. While the foreground window keeps changing, a composition that changed only other windows joins its latest picture instead of starting one, and so does a composition with no dirty region (WGC draws the cursor without one). This is not the October 26 hold below: that one held the game's own later compositions by their timing, while this one never holds the game's own compositions. A game alone on the display is claimed exactly as before (`a_game_alone_on_the_display_is_claimed_as_without_focus_labels`).

The first build left the pointer out. With the pointer circling outside the game at 125 Hz, its compositions came unlabelled and ended the extension. The claim then stopped waiting for the predicted game frame and took the previous one just before the next arrived, a game frame late (traced: the claimed capture was 4 to 7.7 ms old in 133 of 1137 claims). The fix counts a composition without dirty regions as part of the picture while the foreground window animates (`a_pointer_moving_beside_the_focused_game_does_not_cost_it_a_frame`). The rows below are from that build.

Setup: `e2e.py --codec hevc --vrr --motion-script 60 --record --seconds 60 --config minimum_fps_target=20` with an isolated test host. The motion probe strip (the 60 Hz game) was kept in the foreground, and ffplay drew `testsrc2` at 240 fps in a 480x270 window on the same display. Pointer legs circled the pointer outside the game at 125 Hz. A is main at `843b5e923`, B is `843b5e923` with this change and the pointer fix, each in the rc.5 package. Each leg waited for a quiet CPU first. Other lanes' builds still loaded some legs: the CPU column is the machine's load during the stream, and legs at 75 % or more (A at 98 and 99 %, B at 91 and 99 %, one A leg failing on delivery gaps) are left out. Repeats are game frames the client's motion probe showed twice. Age and present-to-sent are the host's 5 s `stream timings` lines, averaged over the leg. Picture age is the client's.

| Leg | Pointer | CPU | Repeats / skips | Age p50 / p95 / p99 | Present-to-sent p50 / p95 / p99 | Cadence error p50 / p99 | Picture age mean / p95 | Pictures/s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| A | moving | 40 % | 200 / 194 | 0.28 / 3.31 / 6.22 | 3.25 / 5.68 / 8.72 | 0.24 / 9.34 | 19.1 / 26.5 | 176.9 |
| A | moving | 36 % | 159 / 154 | 0.28 / 2.78 / 7.63 | 3.22 / 5.32 / 10.28 | 0.26 / 9.07 | 20.0 / 27.5 | 190.7 |
| B | moving | 50 % | 0 / 0 | 0.30 / 0.39 / 0.43 | 2.82 / 3.26 / 3.42 | 0.17 / 0.84 | 12.8 / 14.7 | 60.0 |
| B | moving | 29 % | 4 / 1 | 0.47 / 1.52 / 1.72 | 3.07 / 4.17 / 4.51 | 0.24 / 2.61 | 15.2 / 19.3 | 60.3 |
| B | moving | 50 % | 20 / 14 | 0.80 / 4.85 / 6.06 | 3.90 / 7.71 / 8.73 | 0.24 / 4.88 | 17.7 / 25.2 | 63.9 |
| A | still | 9 % | 294 / 289 | 1.24 / 15.23 / 16.46 | 6.20 / 17.59 / 18.85 | 1.43 / 16.33 | 17.4 / 25.8 | 105.3 |
| B | still | 29 % | 0 / 0 | 0.26 / 0.35 / 0.39 | 2.76 / 3.22 / 3.37 | 0.18 / 0.88 | 12.1 / 13.2 | 60.0 |

With the pointer moving, B's age p95 averaged 2.25 ms against A's 3.05, p99 2.74 against 6.93, and present-to-sent p95 5.05 against 5.50, with 0 to 20 repeats against 159 to 200. One B leg, run while another lane's build used half the CPU, had a higher age p95 than either A leg (4.85 ms). With the pointer still, B was ahead on every column. In that pair ffplay reached only about 110 compositions a second against 240 in the earlier legs, on both builds. On October 29, without the pointer fix, two pairs without the pointer also favoured B: repeats 3.5 against 53 per leg, age p95 / p99 0.71 / 0.94 against 1.01 / 6.32 ms, present-to-sent p95 3.26 against 3.99 ms. **Kept.**

## October 29: Nightjar desktop on a VRR panel and a fixed one

Until now the VRR evidence was the host's: capture under the VRR hint and arrival at the client. This is the first measurement of how Nightjar desktop shows a VRR stream. VRR is on by default and presents on decode with tearing allowed. On a display that keeps its own refresh, that tears. Nightjar now runs the beam race's display check (the scan line around each present) on every VRR stream that tears in fullscreen. On Windows, a display that scans at its own rate gets V-Sync when the stream runs below about 70% of that rate. Only 60 fps was measured; a faster stream on such a display still tears, because its presents leave a VRR display no idle time either.

Fixture: this PC (RX 7900 XT, Samsung G9 5120x1440, FreeSync 48-240 Hz) ran an isolated 3.0.0-rc.5 test host on 127.0.0.1 through `client/scripts/pyrowave-loopback.ps1`, with the scrolling content of `pyrowave-anim.ps1`. Nightjar desktop ran from `C:\tests\nightjar\a` in borderless fullscreen (branch build `f3d2b5393`), PyroWave 1080p60 at 399 Mbps with GPU decode, and PresentMon 2.6.0 on the client. Each arm got one 60 s leg; the first 300 frames are dropped.

The laptop (Radeon 780M, fixed 120 Hz 2880x1800) ran the same build from `C:\tests\dn-nj`, HEVC 1080p60 at 20 Mbps over Wi-Fi. Its host was an rc.2 test host on this PC, because the rc.5 exe has no firewall rule.

Columns:

- **Mid-scan**: the scan line at the present call was outside the blanking. A tearing present there shows the new picture from the beam down.
- **receive_to_present**: the frame received to the present call returned.
- **Until displayed**: PresentMon's `MsUntilDisplayed`, from the present call to the display.

**The display check (D1).**

| Leg | Display check | Pacing | Mid-scan | receive_to_present p50 / p95, ms | Until displayed p50 / p95, ms |
| --- | --- | --- | ---: | --- | --- |
| G9, check off (`NIGHTJAR_VRR_CHECK=0`) | none | Immediate | not read | 4.39 / 5.11 | 0.52 / 1.49 |
| G9, check on (default) | on at 2.6 s | Immediate | 12.3% | 4.27 / 5.13 | 0.50 / 1.30 |
| Laptop, check on (default) | no VRR at 2.2 s after the first frame | V-Sync from then | 97.8% before the switch | | |

- **Cost:** the check added 0.02 ms at p95 receive_to_present.
- **The G9's calibration:** it read 12% of the scan line in the blanking. The panel was still scanning at its top rate, as one calibration in eight did on October 26. The restart check passed (30 tested, 0 missed), so the stream stayed Immediate.
- **Why the rule waits:** a rule on the calibration alone would have put this PC on V-Sync. The kept rule also waits for 60 presents that start no scan.
- **The laptop:** it read 1% in the blanking, and no present started a scan. It switched at frame 134, logged `pacing: V-Sync, this display does not refresh on present`, and the overlay says so on its Pacing line.

**Presenting while the panel still scans (D2, `NIGHTJAR_VRR_SYNC`).** These legs ran with `NIGHTJAR_BEAM_RACE=1 NIGHTJAR_BEAM_RACE_PRESENT=0`, the October 26 beam-race setup:

- `present1`: sync interval 1 while VRR is on.
- `scan`: while the beam is in the picture's rows, wait until it leaves the last one, then present with tearing.

"Received to shown" adds the two percentiles, so its p95 is an upper bound. The 240 fps legs ran at the client's 530 Mbps for that rate. The host delivered about 90 of the 240 fps.

| Arm | Shown fps | Mid-scan | receive_to_present p50 / p95, ms | Until displayed p50 / p95, ms | Received to shown p50 / p95, ms |
| --- | ---: | ---: | --- | --- | --- |
| off, 60 fps | 60.4 | 7.6% | 3.81 / 4.70 | 0.46 / 1.19 | 4.27 / 5.89 |
| present1, 60 fps | 59.3 | 0 (9.2% waited for the scan) | 4.20 / 5.10 | 0.55 / 8.28 | 4.75 / 13.38 |
| scan, 60 fps | 46.7 | 0 (574 waits, 2.11 ms each) | 4.53 / 7.51 | 0.49 / 1.21 | 5.02 / 8.72 |
| off, 240 fps | 96.3 | 3.4% | 1.96 / 2.51 | 0.45 / 1.06 | 2.41 / 3.58 |
| scan, 240 fps | 87.7 | 0 (140 waits, 2.40 ms each) | 2.03 / 2.62 | 0.45 / 1.00 | 2.48 / 3.62 |

- **present1:** PresentMon saw every present as "Hardware Composed: Independent Flip" (off: 85% "Hardware: Independent Flip"). Its until-displayed p95 rose from 1.19 to 8.28 ms.
- **scan:** it stopped the mid-scan presents but added 0.75 ms at p50 and 2.8 ms at p95 at 60 fps. In that leg the host delivered 47 fps, and a slower, irregular rate meets more scans that the panel starts on its own below 48 Hz. At 240 fps it stayed within the bounds: +0.07 ms at p50 and +0.04 ms at p95, with mid-scan presents going from 3.4% to 0.
- **Result:** the bar for a default was under 1% mid-scan presents, and received to shown within 0.2 ms at p50 and 0.5 ms at p95 of off. Neither arm passes it at 60 fps, so both stay off. `NIGHTJAR_VRR_SYNC` stays a measurement switch.
- **Not run:** the 240 fps scan result is one leg and was not repeated. Exclusive fullscreen keeping the panel's top refresh (D3) has unit tests only. Running it would have switched the mode of the main monitor while the user was at it.

## October 29: false slow-pace latches and a stalled adapter (rc.4 field logs)

#59's rc.4 logs showed three ways a clean wired stream fell to the Wi-Fi pace (twice the bitrate) for the rest of the session and for 24 hours: Moonlight's decoder asking twice for a keyframe with no loss reported, the host's own adapter refusing packets (`10035`, WSAEWOULDBLOCK) while a game started, and an AV1 recovery keyframe of 1.91 MB against a 1.17 MB cap. The fixes kept are: a client that announces FEC status (featureFlags 0x01) no longer latches on a repeated request (commit 1); a latch made only by frames lost on send lifts after 10 clean seconds and 120 frames and is never saved (commit 2); and keyframes over 1.1 times the automatic cap lower the cap AMF gets, which holds every frame, back to the full cap for good when a lower cap does not make the next keyframe smaller (K, commit 6). The candidate W, a full socket holding a large frame up to its own wire time instead of dropping it (commit 4), measured worse in P5 below and was reverted. `bottleneck.py` gained `--idr-probe N:MS` (the receiver asks N times per keyframe, MS apart) and `--send-outage EVERY_MS:LENGTH_MS[:stall]` (the test host refuses video sends; `stall` fills the socket like a stalled adapter). Fixture: the RX 7900 XT host, the loopback receiver with D3D11VA, 45 s per run, one binary per arm (`pkg-c` the base 545d678c1, `pkg-h` commits 1-3, `pkg-w` 1-4, `pkg-i` 1-5, `pkg-k` 1-6), C,R,R,C on AV1 3840x2160x100 at 219.5 Mbps (a 20 s calibration run held 100.0 fps) and C,R on HEVC 2560x1440x120 at 150 Mbps. The receiver is not a paired device, so no arm saves a loss record: the record rules are checked by `keyframe_loss` and `state::loss_memory` tests (`send_loss_alone_is_not_remembered`, `an_older_sustained_record_is_not_read`, `a_client_report_confirms_a_send_latch_for_memory`).

**P1, the decoder asks twice** (`--idr-probe 2:100`, C `pkg-c`, R `pkg-i`). Host present-to-send is the mean of the second to fourth 5 s `stream timings` windows.

| Arm | Latch | Requests not counted | Stalls | Picture age p50 / p99, ms | Host present-to-send, ms | Encode mean, ms |
| --- | --- | ---: | ---: | --- | ---: | ---: |
| AV1 C, run 1 | at 2.8 s, rest of stream | 0 | 0 | 14.93 / 21.76 | 7.62 | 5.31 |
| AV1 R, run 1 | none | 1 | 0 | 15.14 / 22.33 | 7.12 | 5.36 |
| AV1 R, run 2 | none | 1 | 0 | 15.49 / 22.12 | 7.33 | 5.39 |
| AV1 C, run 2 | at 2.7 s, rest of stream | 0 | 0 | 14.86 / 21.60 | 7.13 | 5.32 |
| HEVC C | at 2.8 s, rest of stream | 0 | 0 | 11.78 / 13.78 | 3.48 | 3.18 |
| HEVC R | none | 1 | 0 | 11.71 / 13.91 | 3.42 | 3.19 |

R never latched. AV1 picture age p50 was 0.42 ms higher on R's mean, over the 0.3 ms bound we set; p99 was 0.55 ms higher (bound 1 ms), and HEVC was within both. The gap is not on the host: present-to-send was 0.15 ms lower on R, the receiver's per-frame host processing (0.4-0.5 ms), transfer (0.30-0.33 ms) and decode (3.6-3.7 ms) medians matched, and P2's first 20 s, where both arms ran the same policy, had R 0.56 ms higher too. We read it as the receiver's present phase between runs, not the change. A check pair without fault injection, 60 s each, the base against the branch tip (commits 1-3, 5 and K, without W): picture age p50 14.30 against 14.29 ms, p99 20.71 against 20.86 ms, receiver host processing p50 0.5 ms on both, present-to-send 7.05 against 6.95 ms, encode mean 5.20 against 5.19 ms. Kept.

**P2, the host's socket refuses video** (`--send-outage 20000:30`: 30 ms of refused sends at 20 and 40 s, C `pkg-c`, R `pkg-i`).

| Arm | Latches (s) | Lifted (s) | Stalls | Whole | Picture age p50 / p99, ms |
| --- | --- | --- | ---: | ---: | --- |
| C, run 1 | 20.0 | none | 2 | 99.98% | 15.93 / 41.81 |
| R, run 1 | 20.0 (send), 40.0 (rest of stream) | 30.0 | 2 | 100.29% | 16.47 / 83.22 |
| R, run 2 | 19.9 (send), 39.9 (rest of stream) | 30.0 | 2 | 100.25% | 15.82 / 36.47 |
| C, run 2 | 20.0 | none | 2 | 100.02% | 15.25 / 21.97 |

R lifted the send latch 10 s after the first outage and held the pace for the session after the second, as designed. R run 1's p99 was already 86 ms in its first 20 s, before the arms differ. Kept.

**P3, the October 14 slow port** (AV1 2560x1440x60 at 40 Mbps through a 100 Mbps shaper with a 64 KiB queue, software decode, no `--rfi`). Neither arm lost video or latched: C 100.07% whole, 0 stalls, picture age 13.37 / 22.17 ms; R 100.00%, 0 stalls, 9.58 / 22.43 ms. This fixture no longer reproduces October 14's latch, so it shows no regression but does not prove the slow port still latches through FEC status alone; `a_client_that_reports_its_losses_still_latches_on_an_incomplete_keyframe` covers that rule. Kept.

**P4, the keyframe cap scale** (`--cut-ms 2000`, C `pkg-i`, R `pkg-k`). No keyframe in C came out over 1.1 times its cap target (AMF's own count, `keyframes_over_cap`, was 0 too), so the overshoot does not reproduce on this card. K stayed inert: `keyframe_cap_scale` was exactly 1 in all 24 windows of R's three runs. Encode mean 5.488 and 5.541 ms (C) against 5.515 and 5.546 ms (R); ordinary picture age p50 11.61 and 11.60 against 11.71 and 11.72 ms, p99 49.56 and 48.77 against 47.87 and 48.20 ms; HEVC 3.185 ms encode on both, ordinary 11.69 / 13.61 against 11.64 / 13.74 ms. Kept on its unit model (`a_proportional_overshoot_lands_within_the_cap_in_two_keyframes`, `a_large_overshoot_floors_the_scale_and_the_cap_at_two_frames`, `a_cap_the_driver_ignores_for_keyframes_returns_the_scale_to_one_for_good`, `each_keyframe_cap_relax_step_reaches_the_driver`, `the_clamped_last_relax_step_back_to_the_target_reaches_the_driver`; a driver that ignores the lower cap is logged once); what it does for the RDNA4 overshoot in #59 is not measured here.

**P5, a stalled adapter** (`--cut-ms 2000 --send-outage 3000:20:stall`: the socket stays full for 20 ms every 3 s, C `pkg-h`, R `pkg-w` with W). Presentation gaps are the receiver's times between presented frames; "frozen" sums each gap's excess over 10 ms.

| Arm | Lost on send | Send-loss recoveries | Stalls | Picture age p50 / p99, ms | Ordinary p50, ms | Longest gap, ms | Frozen, ms |
| --- | ---: | ---: | ---: | --- | ---: | ---: | ---: |
| AV1 C, run 1 | 22 | 22 | 11 | 11.74 / 51.70 | 11.72 | 36.0 | 1,863 |
| AV1 R, run 1 | 0 | 0 | 0 | 11.91 / 64.97 | 11.88 | 45.6 | 2,258 |
| AV1 R, run 2 | 0 | 0 | 0 | 12.48 / 59.68 | 12.46 | 46.2 | 2,113 |
| AV1 C, run 2 | 22 | 22 | 11 | 11.95 / 52.05 | 11.93 | 35.8 | 2,032 |
| HEVC C | 2 | 2 | 1 | 12.29 / 17.45 | 12.28 | 25.2 | 174 |
| HEVC R | 0 | 0 | 0 | 12.11 / 14.86 | 12.10 | 25.0 | 131 |

W delivered every frame and took no latch, but the AV1 frames it held waited out the stall, and the frames behind them with them: p99 rose by 10 ms and the longest gaps by about 10 ms. Losing the frame and sending a recovery frame at once froze the picture for less (the no-stall P4 runs had longest gaps of 36-41 ms). HEVC was slightly better with W; the keep rule needs both. Not kept (reverted).

## October 28: the console's Stream page against the browser page (D4)

The rc.4 pass failed D4 on glass-to-glass: `webrtc_e2e.cjs --console` measured the console's Stream page at a 21 ms median (p99 25) against 12 ms for the browser client a minute earlier, over the 5 ms gate. The page is not slower. Same fixture as the WebRTC sections below (test host on `C:\tests\webrtc`, headless Edge over loopback, H.264 1080p60, 3.0.0-rc.4 package), one 60 s stream per leg, legs run one after the other:

| Run | Browser page median, ms | Console page median, ms | Up to arrival, browser / console, ms | Arrival to display, browser / console, ms |
| --- | ---: | ---: | --- | --- |
| rc.4 pass D4 | 12 | 21 | not split | not split |
| Before (same harness, same pages) | 21 | 21 | not split | not split |
| After 1 | 17 | 17 | 14.5 / 14.2 | 2.0 / 1.9 |
| After 2 | 21 | 17 | 18.8 / 14.7 | 1.6 / 3.1 |

A diagnostic stream on the console page (12 s phases on one stream) moved the median no more than within noise: 21 ms as it is, 19 without the picture's rounded clip, 25 with every animation off, 21 with only the player visible, 19 in full screen; the browser page in the same run measured 21 in full screen and 21 out of it. The probe now splits each sample at the frame's arrival (`requestVideoFrameCallback` `receiveTime`). From arrival to display both pages take 2–3 ms. Up to arrival (host, network, the clock window) the median moves by itself between streams, by 4 ms in one run and probably the 9 ms of D4, in both directions, while the host's encode and present-to-send stay the same. The gate now holds the console page's part, arrival to display, to the browser page's plus 5 ms. A gap before arrival goes into the run's notes with each leg's host timings. No change to the page.

## October 28: oversize scene cuts encoded again as keyframes

The cuts the frame cap above could not save: AMF picks each frame's QP from the frames before and never encodes again, so after steady 385 KB frames a cut comes out as an inter frame of about 2.4 MB, past the 1.17 MB that keeps full parity in four blocks, and is lost. A keyframe of the same picture lands at the cap (855 KB). Now an inter frame larger than the stream's full-FEC bound (the same bound the cap is held to: `network_pacing::fec_whole_frame_limit`; none where two nominal frames already pass it, as in the cap's decision below, which these runs at about 830 KB for two frames are not) is not sent: its converted input goes back to AMF once as a keyframe at the same wire frame, and that keyframe is sent. There is no lookahead and no added frame of delay; the second encode happens only on such frames. Same fixture as the section below (HEVC 1440p60, 250 Mbps, 0.3% random loss, a cut every 2 s, `--fresh-rows 256`, 45 s), C = the cap within full FEC with LTR repair on (the merged branch), R = C plus the re-encode; two pairs, C then R and R then C, one binary each.

| Arm | Lost cuts | Re-encoded | Freezes (over 100 ms) | Whole | Keyframe requests | Reference invalidations | End to end p50 / p95 / p99, ms | Encode mean / p99, ms | Cut keyframe picture age p50 / max, ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- | --- |
| C, run 1 | 16 | 0 | 15 (0) | 99.81% | 15 | 19 | 20.09 / 36.67 / 71.85 | 4.04 / 4.76 | 44.5 / 115.0 (recovery) |
| R, run 1 | 0 | 14 | 0 (0) | 100.38% | 1 | 0 | 19.19 / 23.91 / 34.09 | 4.00 / 4.63 | 37.2 / 89.0 |
| R, run 2 | 0 | 14 | 0 (0) | 100.07% | 1 | 0 | 18.33 / 20.69 / 29.37 | 4.02 / 4.77 | 35.4 / 51.6 |
| C, run 2 | 15 | 0 | 15 (0) | 99.93% | 16 | 17 | 17.98 / 20.92 / 30.06 | 3.96 / 4.74 | 41.7 / 68.6 (recovery) |

"Lost cuts" are the client's unrecoverable frames; "whole" is the harness's frames received over frames sent, as it reports them. Encode is the mean of the 5 s `stream timings` windows. Of about 22 cuts a run, 14 came out past the bound and were encoded again; the rest (inter frames up to 950 KB) kept full parity and went as they were. Every freeze is gone with them, and with them the keyframe requests (15–16 to 1, the startup one) and the reference invalidations. The overall encode time did not move: the second encode falls on 14 of about 2,700 frames. The host does not time a re-encoded frame on its own, so the encode time on cut frames is not measured; the cut keyframe arrives 5–7 ms sooner (picture age p50) than C's recovery keyframe, and 2.4 MB at 800 Mbps would take about 24 ms to send against about 9 ms for the 855 KB keyframe. No re-encode failed. End to end p99 was 34.1 against 71.9 ms and 29.4 against 30.1 ms. A re-encode happens only when nothing submitted later references the inter frame, never while an input waits on a full AMF queue, and never with slice delivery (the frame's first slices may have left). The keyframe resets the long-term references, so a later LTR repair names only the keyframe or frames after it (`ltr::tests::a_keyframe_re_encoding_an_accepted_frame_drops_every_older_reference`); `amf::tests::native_an_oversize_scene_cut_goes_out_once_as_a_keyframe_at_its_own_index` checks H.264, HEVC and AV1 on the GPU (one output a frame, keyframes at 1 and the cut, one re-encode, a repair after it). Explicit `amd_max_frame_size` values keep AMF's output as it is. Kept.

## October 28: lost scene cuts and recovery keyframes past four FEC blocks

A wired client at about 930 Mbps reported stutter at every scene change at 250 Mbps, with the host's `network_fec` warning ("FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit"). With the test client's 1024-byte packets and the default 20% FEC, Moonlight's four blocks carry full parity up to about 855 KB and no parity above about 1.03 MB (1.17 MB and 1.40 MB with 1392-byte packets); the automatic frame cap (two periods at 800 Mbps, 2–8 nominal frames) allowed 2.46 MB in this run. Fixture: `rust/release/bottleneck.py` on the RX 7900 XT, loopback with the Moonlight test client, HEVC 2560×1440 at 60 fps and 250 Mbps (199 Mbps to the encoder), `--rate-kbps 0 --loss-percent 0.3` (random loss on video datagrams, no bottleneck), `--rfi --cut-ms 2000 --fresh-rows 256 --hw-decoder d3d11va`, 45 s, one run per arm under the GPU lock. Calibration without loss: ordinary frames 385 KB (0.93 of the 415 KB nominal frame), cuts 1.74 MB (4.2 nominal), no freeze. "Freezes" are display gaps from `recovery_gaps.py`; "end to end" is the client's picture age (capture to presentation); "whole" is frames received over frames sent.

| Arm | Freezes (over 100 ms) | Freeze length, ms | Whole | Recovery keyframes | Keyframe requests | Full latch | Bitrate cuts | End to end p50 / p95 / p99, ms |
| --- | ---: | --- | ---: | ---: | ---: | --- | ---: | --- |
| main (gate) | 16 (6) | | 98.98% | 17 | 30 | yes | 14 | 21.67 / 28.47 / 42.15 |
| off | 17 (4) | 75–125 | 99.02% | 18 | 27 | yes | 11 | 20.26 / 26.19 / 29.51 |
| S, oversize-only pacing (4 runs) | 16, 16, 16, 16 (5, 3, 5, 4) | | 99.02–99.25% | 17–18 | 26–27 | yes, all | 10–12 | 20.04–20.29 / 25.96–26.61 / 29.48–30.11 |
| C, cap within full FEC | 15 (0) | 43–57 | 99.63% | 16 | 16 | no | 0 | 17.12 / 23.09 / 26.44 |
| C+S | 16 (0) | 42–77 | 99.40% | 17 | 17 | no | 0 | 17.11 / 23.62 / 27.09 |

Off and S lose each cut, then most of the recovery keyframes sent at the 2.46 MB cap with no parity; the lost keyframes latch the slow pace, the bitrate controller cuts the encoder from 199 to 161–193 Mbps, and freezes run to 125 ms. Arm C holds the automatic cap to the full-parity size at the recovery FEC percentage (855 KB with these packets): every recovery keyframe arrives, nothing latches, the pace stays at 800 Mbps, no freeze passes 100 ms, and picture age drops 3 ms at p50, p95 and p99 because the latch no longer slows every frame. The cut itself is still lost in every arm: AMF does not hold an ordinary P-frame of full-screen noise to the cap (the lost cuts were four blocks of about 433 shards, 1.74 MB, with no parity), so each cut costs one keyframe round trip, about 50 ms. S (pacing only frames over two budgets at the Wi-Fi rate after one is lost) measured the same as off, since random loss does not depend on the pace, and adds nothing to C.

Decision: the automatic frame cap stays within the largest frame that keeps full parity in four blocks, but not below two nominal frames (so streams whose nominal frame already nears the limit, above about 280 Mbps at 60 fps with 1392-byte packets and 205 Mbps with 1024-byte ones, keep today's cap). It applies to H.264, HEVC and AV1 for every client. With a fixed percentage it is the limit at the recovery percentage. Under `rlFecPercentage=auto` it is the largest frame whose own statistical percentage (at least the configured recovery share) still fits in full, found once a second. Without FEC it is the four-block packet limit (1020 packets, about 1.40 MB at 1392 bytes), so a lost cut exposes as few packets as it can. Explicit `amd_max_frame_size` values are unchanged. S is not kept. Test: `keyframe_loss::tests::recovery_keyframes_within_full_fec_survive_random_loss_without_a_latch` models this run.

The other paths, on the same fixture (45 s, one run per arm unless noted, off = main, C = the extended cap):

| Path | Arm | Freezes (over 100 ms) | Whole | Recovery keyframes | Encoder bitrate at the end | End to end p50 / p95 / p99, ms |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| No FEC (`fec_percentage=0`), 0.3% loss | off | 58 (3) | 95.22% | 59 | 2.4 Mbps | 16.65 / 20.58 / 41.08 |
| | C | 266 (119) | 45.91% | 268 | 249 Mbps | 14.44 / 35.37 / 44.16 |
| | C, second run | 64 (3) | 95.74% | 65 | 2.9 Mbps | 16.22 / 20.22 / 40.09 |
| No FEC, 0.05% loss | off | 45 (2) | 97.84% | 47 | 8.2 Mbps | 16.91 / 19.24 / 38.26 |
| | C | 48 (1) | 97.91% | 49 | 7.3 Mbps | 16.94 / 19.74 / 37.82 |

Without FEC, 0.3% loss loses about half of the 375 KB ordinary frames in any arm, and the bitrate controller decides the run: it took off to 2.4 Mbps, but the first C run probed back to 249 Mbps, where ordinary frames died. The second C run followed off down (2.9 Mbps) and matched it, so the first was the controller's path, not the cap; at 0.05% loss the arms match. This client uses 1024-byte packets, so the four-block limit (1.03 MB) sits just under two nominal frames (1.04 MB). These runs held such a stream at the floor: C held recovery keyframes to 1.04 MB where off sent up to 2.95 MB. Kept: not worse. The shipped cap follows the decision above instead and leaves a stream whose two nominal frames already pass the limit at today's cap, so on this fixture without FEC it sends as off did; streams below the limit (1392-byte packets up to about 337 Mbps at 60 fps) get the four-block packet limit. `rlFecPercentage=auto` could not run on this fixture: the Moonlight test client does not choose FEC at launch or send FEC reports, and linkspeed.py (Nightjar on the NUC) has no scene-cut or random-loss arm. Covered by `session::tests::auto_fec_finds_the_largest_recovery_frame_its_own_percentage_covers_in_full`: on a lossy link the average frame's share rises to 40%, but a large recovery frame still fits its 20% in full, so the cap is the 20% limit, not the tighter 40% one. PyroWave needs no cap and has none: its rate control gives every picture, a cut included, at most one frame's budget (`pyrowave::tests::a_scene_cut_gets_no_more_than_one_frame_budget_so_it_needs_no_frame_cap`), below the two-frame floor, and the test client cannot decode PyroWave ("Unknown codec: pyrowave").

## October 27: audio passthrough encode cost

What the host spends encoding AC3 (Dolby Digital) and E-AC3 (Dolby Digital Plus) for a client that asks for passthrough, against the Opus it sends otherwise, and what the formats cost in latency. The fixture is the ignored test `passthrough_encode_cost` in `rust/windows/src/ffmpeg/tests.rs`: 32 s of a 440 Hz tone over noise on every channel, each packet timed on its own, release build (`cargo test --release -p butterpollo-windows --lib -- --ignored --nocapture passthrough_encode_cost`), FFmpeg 8.1.2 and the MSYS2 libopus, Ryzen 7 5800X3D, branch `rl-pass` (2026-10-27). One run; the PC was in use but not gaming, so the p99 and largest values carry scheduler noise and only the medians compare.

| Codec | Channels, bitrate | Packet | Encode median / p99 / max (µs) | Encode per second of audio | Codec delay |
| --- | --- | ---: | --- | ---: | ---: |
| AC3 | 2, 192 kbit/s | 32 ms | 75.2 / 421.2 / 16,507.5 | 2.4 ms | 256 samples (5.3 ms) |
| AC3 | 6, 640 kbit/s | 32 ms | 162.2 / 732.6 / 4,457.6 | 5.1 ms | 256 samples (5.3 ms) |
| E-AC3 | 2, 256 kbit/s | 32 ms | 70.8 / 321.4 / 1,334.1 | 2.2 ms | 256 samples (5.3 ms) |
| E-AC3 | 6, 768 kbit/s | 32 ms | 196.1 / 1,321.3 / 5,917.8 | 6.1 ms | 256 samples (5.3 ms) |
| Opus | 2 | 5 ms | 61.2 / 692.8 / 21,880.7 | 12.2 ms | not measured |
| Opus | 6 | 5 ms | 159.5 / 1,687.3 / 45,369.0 | 31.9 ms | not measured |

Encoding is not where the latency goes: a 32 ms AC3 frame takes about the time of one 5 ms Opus packet, and per second of audio the Dolby encoders use about a fifth of the CPU Opus uses. The codec delay is where a click placed in the second frame comes back out of FFmpeg's decoder, the same 256 samples for all four.

The cost is the format's framing, and it is inherent: an AC3 or E-AC3 frame holds 1,536 samples, so the host waits 32 ms for a frame where Opus waits 5 ms. A sound reaches the network up to 27 ms later than with Opus (13.5 ms on average, at a uniform position in the frame), plus the 256-sample codec delay against Opus's 2.5 ms look-ahead (its specified value, not measured here), so about 30 ms at worst before the client. The receiver, soundbar or TV then buffers and decodes whole frames, which adds its own delay on top; no sink has been measured yet (user case `audio-passthrough-tv` in `tools/hw/catalog.yaml`). PCM passthrough has no encoder and no codec delay, in 5, 2 or 1 ms packets for 2, 6 or 8 channels. This is why Opus stays the default and the client setting says Dolby audio arrives later.

## October 27: client error correction and a link test before the first bitrate (NUC)

What the client-chosen FEC share (`rlFecPercentage`, the 0x5539 loss report) and a `GET /bandwidth-probe` before the first bitrate do under shaped loss. Harness `rust/release/linkspeed.py` with the NUC's nightjar-cli as the client (`LINKSPEED_CLIENT_FEC`, `LINKSPEED_CLIENT_PROBE`), rc.3 package with the B3 host build, RX 7900 XT, HEVC, 45 s per run, 3 runs per arm, the PC idle, one arm at a time under the GPU lock. The host log confirmed the client's choice (`client fec choice ... choice=auto` / `choice=20`). Run-to-run variance is high: host arms lost between 0.5 and 3 %. The Android unit tests ran beside the two link-test arms, so those may carry CPU contention.

| Arm | Lost % (3 runs) | Freezes | IDR requests | Picture age p50 / p99 ms | Bitrate changes |
| --- | --- | --- | --- | --- | --- |
| wifi 20 Mbps, host's adaptive FEC (stayed at 20 %) | 2.09 / 0.74 / 1.06 | 12 / 2 / 8 | 20 / 9 / 11 | 21.0-26.0 / 42-49 | 10 / 7 / 0 |
| wifi 20 Mbps, client auto (went to 50 %) | 0.52 / 1.01 / 1.14 | 1 / 8 / 8 | 7 / 11 / 14 | 16.0-18.1 / 31-32 | 0 / 0 / 0 |
| wifi-hi 60 Mbps, host's adaptive FEC | 2.37 / 3.02 / 1.63 | 24 / 21 / 19 | 22 / 23 / 17 | — / 50-56 | 6 / 10 / 7 |
| wifi-hi 60 Mbps, client auto | 0.17 / 1.71 / 0.26 | 1 / 7 / 1 | 5 / 8 / 4 | — / 32-36 | 0 / 5 / 0 |
| wifi-hi, host fixed 20 % on top | 0.74 / 0.52 / 1.52 | 2 / 1 / 3 | 7 / 6 / 17 | 19.1-20.3 / — | 0 / 0 / 6 |
| wifi-hi, client 20 % inside the budget | 1.26 / 1.04 / 1.29 | 3 / 2 / 13 | 14 / 10 / 16 | 21.3-22.6 / 35-46 | 4 / 4 / 7 |

Client auto beat the host's adaptive FEC on every count: on wifi `compare` PASSed (p50 -4.9 ms, p99 -15.3 ms); on wifi-hi it failed only on its no-bitrate-change rule (5 change lines in one run) with p50 -0.63 ms, p99 -18.1 ms, median loss 2.37 → 0.26 % and freezes 21 → 1. The wifi-hi encoder held 38-47 Mbps against 26-36. The fixed 20 % share inside the budget measured worse than the host's 20 % on top (median loss 1.26 against 0.74 %, p50 +1.72 ms, p99 +3.27 ms) and sent 15-21 Mbit/s against 19-26.

The link test on the 50 Mbit/s `r50-hevc-hi` link measured 45.6-48.0 Mbit/s in 0.70-0.74 s (4 MiB) and started the stream at 36.5-38.4 Mbps instead of 75. `compare` PASSed (p50 -0.07 ms, p99 +2.26 ms), but the tested arms lost 0.05 / 1.08 / 0 % against 0 / 0 / 0, froze 13 / 8 / 4 times against 2 / 6 / 7, and their encoder averaged 28-30 Mbps against 59; both arms sent 22-27 Mbit/s. `loss_first5s_percent` was not reported. The link test measured worse here. Caveat: a Gradle build (the Android unit tests) ran on this PC alongside both link-test arms, so they may carry CPU contention the untested arm did not; it has not been rerun without the build.

Decision: the client's Error correction setting defaults to the PC's setting, with auto as the better choice measured on these links; fixed shares stay options. Client auto stays opt-in until a real Wi-Fi run on a phone. The link test before the first bitrate measured worse, so it ships as an Experimental setting that is off by default ("Link test before the first bitrate", `checkbox_link_probe` on Android, `link_probe` on desktop).

## October 27: AV1 intra refresh and recovery after a lost frame

What negotiated AV1 intra refresh does on AMF when a frame is lost, and why the host now leaves it off. The fixture is the ignored native test `native_av1_intra_refresh_recovery_after_a_lost_frame` (report path in `BUTTERPOLLO_TEST_AV1_INTRA_REPORT`, ffmpeg in `BUTTERPOLLO_TEST_FFMPEG`): RX 7900 XT, driver 32.0.32015.7029, AV1 2560×1440 at 120 fps and 50 Mbps (a 52,083-byte frame budget), a synthetic texture scrolling 16 pixels a frame with no capture, display or network, 900 frames paced at 120 Hz, one run per arm under the GPU lock with the PC idle. Frame 480 is left out of a copy of the stream, and frame 481 carries the host's answer to the loss: nothing, a keyframe, or the invalidation of the lost frame. ffmpeg 9.0.2 with dav1d decodes both copies; a frame after the loss counts as recovered when its picture matches the full stream's (MD5). Debug test build; the byte counts come from the encoder.

| Arm | Keyframes (bytes) | Frame 481 (bytes) | Largest frame after the first | Mean inter frame | Lossy copy decoded | Recovered from |
| --- | --- | --- | ---: | ---: | --- | --- |
| refresh, nothing sent | 1 (203,795) | 57,146, inter | 60,682 | 51,518 | 476 of 899 | never (419 frames to the end) |
| refresh, keyframe | 2 (203,795 / 101,162) | 101,162, keyframe | 101,162 | 51,203 | 899 of 899 | frame 481 |
| no refresh, invalidation | 1 (203,795) | 47,592, inter | 66,072 | 33,515 | 899 of 899 | frame 481 |
| no refresh, keyframe | 2 (203,795 / 99,990) | 99,990, keyframe | 99,990 | 36,067 | 899 of 899 | frame 481 |

The driver reads back `Av1IntraRefreshMode` 2 and 300 stripes with refresh and turns long-term references off (`Av1MaxNumLTRFrames` 0, against 4 without refresh), so it refuses the host's invalidation and a loss costs a keyframe. `GetPropertyInfo` fails for both refresh properties while `GetProperty` reads them.

Refresh alone never recovers: AMF's AV1 stream signals frame IDs (`frame_id_numbers_present_flag`), and every refresh frame refers to and replaces reference slot 0, so after the loss dav1d rejects every frame header to the end of the run. With the keyframe the host sends, recovery is immediate but costs a 101,162-byte keyframe where the invalidated frame without refresh is 47,592 bytes, and inter frames average 51,203 bytes against 33,515 (without refresh the encoder stays under its budget on this source). On this driver refresh measured worse on every count.

Decision: AMF AV1 no longer asks for intra refresh. A client that negotiates it gets the stream of one that never asked, keeps its long-term references and recovers from a loss with the invalidated frame (the "no refresh, invalidation" row). H.264 and HEVC refresh are unchanged. The refresh arms now force it with a test-only switch (`intra_refresh::force_av1`) so the measurement repeats; every arm negotiates refresh, and the invalidation arm fails unless the invalidated frame, not a keyframe, restores the picture. Rerun the same day with the policy in place: every arm gave the byte counts in the table, and the default path, asked for refresh and left without it (`Av1IntraRefreshMode` 0, `Av1MaxNumLTRFrames` 4), had its invalidation accepted and matched the full stream from frame 481.

## October 27: HDR10+ and HDR Vivid dynamic metadata

With `hdr_dynamic_metadata` on (the default), the compute conversion measures each HDR frame's maxRGB statistics on the GPU once the frame has gone to the encoder, and the frames AMF puts out from then on carry them: HDR10+ (SMPTE ST 2094-40) in HEVC and AV1, and HDR Vivid (CUVA 005.1) in HEVC. Nothing waits for a sample, so a frame carries the statistics of an earlier one. Measured on the RX 7900 XT, driver 32.0.32015.7029, Windows 11 build 26200, with the release `butterpollo.exe` and `amf_quality` of branch `rl-hdr` at `179046747` (3.0.0-rc.3 plus the feature). Isolated test hosts on their own port, the installed service idle and untouched, no encoder hang. This PC's main display has HDR off, so the streamed desktop is SDR content inside PQ, with flat statistics. The scene-cut values below come from a synthetic source instead: a dark scene (peak 20 nits) and then, from frame 60, a bright one (peak 1000 nits, with a moving highlight).

### The units in AMF's output

`amf_quality`, 1280x720 at 60 fps, 20 Mbps, 120 frames of the synthetic source, target display 1000 nits; each codec encoded with the setting on and off (HEVC off twice). Read with ffprobe from FFmpeg 9.0.2.

| Codec | Frames with HDR10+ | Frames with Vivid | On, our units removed, against off | Dark scene: maxSCL / average | Bright scene: maxSCL / average |
| --- | --- | --- | --- | --- | --- |
| HEVC | 119 of 120 | 119 of 120 | byte-identical (the two off runs too) | 19.6 / 6.2 nits | 1000 / 327 nits |
| AV1 | 119 of 120 | none (Vivid defines no AV1 carriage) | byte-identical | 19.6 / 6.2 nits | 1000 / 327 nits |

Vivid's maximum maxRGB reads 1460/4095 in PQ (19.9 nits) in the dark scene and 3079/4095 (1000.6 nits) in the bright one. **The units add nothing else to the bitstream, and the values follow the source within 2 %.** Frame 0 carries none, because no sample has finished yet. The cut at frame 60 shows from frame 61, one frame later; the metadata on frame 60 is still the dark scene's.

hdr10plus_tool 1.7.2 detects the metadata (`--verify`), but its `extract` refuses both files. First, frame 0 has no unit ("119 vs 120"). With frame 1's unit copied into frame 0 of a scratch copy, it still fails: "Invalid targeted_system_display_maximum_luminance for profile A, must be zero". With `--skip-validation`: "Invalid profile B: no Bezier curve data". We write a nonzero target display (the client's peak) without a tone-mapping curve, which matches neither of the tool's profiles. FFmpeg parses every unit.

### What a Moonlight client receives

Two 20 s streams to the test client (moonlight-common-c `2600bea`) at 1920x1080x60, with every received access unit recorded (`BUTTERPOLLO_TEST_BITSTREAM`). The client reported a 400-nit peak and the host targeted it. The host spliced 1110 of 1111 HEVC frames and 1159 of 1160 AV1 frames.

- **AV1: ffprobe shows HDR10+ on 1154 of 1155 received frames**, target 400 nits, maxSCL 100 nits (SDR white in PQ) and average 10.8 nits on every frame.
- **HEVC: the received stream has no SEI at all**, neither ours nor AMF's mastering-display SEI. moonlight-common-c drops every prefix SEI NAL unit in front of a frame's slices (`VideoDepacketizer.c`, both the IDR and the fast path) before the decoder sees it, so a client built on it never gets HEVC HDR10+ or Vivid. Static HDR10 metadata reaches it through Moonlight's own control message as before.

Both streams decoded every frame. e2e failed them on three long frame intervals each (at most 193 and 659 ms), because they ran during a full build gate.

### CPU cost

From the host's "HDR dynamic metadata" summary line at each stream's end: the CPU time per frame to submit the statistics pass, to read and smooth a finished sample, and to splice the units into the frame. Mean (maximum) in µs.

| Stream | Frames | Submit | Sample | Splice |
| --- | --- | --- | --- | --- |
| `amf_quality` HEVC, 720p60 | 120 | 62.8 (219) | 23.9 (43) | 0.7 (1) |
| `amf_quality` AV1, 720p60 | 120 | 66.0 (366) | 35.7 (243) | 1.3 (23) |
| Stream HEVC, 1080p60 | 1111 | 53.6 (185) | 33.0 (77) | 3.9 (60) |
| Stream AV1, 1080p60 | 1160 | 51.1 (246) | 29.4 (267) | 3.4 (18) |
| Stream HEVC, 4K60, four runs | 3568 to 3598 | 39.4 to 51.1 (103 to 412) | 24.1 to 30.6 (40 to 274) | 2.7 to 3.8 (61 to 111) |

**About 60 to 90 µs of CPU per frame, 4 to 5 ms a second at 60 fps.** No run had an invalid sample, a readback error or a frame without a place for the units.

### Encode stage on and off at 4K60 (ABBA)

HEVC HDR 3840x2160x60, 80 Mbps, 60 s per run with the timing record (`--record`), the first 5 s dropped, the stage table from `timing.stages` in result.json. Arms in the order on, off, off, on; off is `hdr_dynamic_metadata = false`. Other agents' `-j 4` cargo checks kept running, but no full build gate did; the CPU load is the mean of 5 s samples of `Win32_PerfFormattedData_PerfOS_Processor` over each run. The test client's software decoder cannot hold 4K60 on a loaded CPU, so its frame rate tracks the load, not the arm. The host's own `stream timings` held 59.7 to 59.9 fps in every run. Set B's sampler failed (a counter path that is localized on this Windows) and spun on one core.

| Set / arm | CPU load | Client fps | Encode p50 / p95 / p99 | Convert p50 / p95 |
| --- | --- | --- | --- | --- |
| B, on | not sampled | 45.4 | 5.830 / 6.329 / 6.749 | 0.337 / 0.396 |
| B, off | not sampled | 39.0 | 5.864 / 6.313 / 6.735 | 0.397 / 0.469 |
| B, off | not sampled | 46.4 | 5.861 / 6.398 / 6.815 | 0.317 / 0.377 |
| B, on | not sampled | 42.7 | 5.876 / 6.335 / 6.772 | 0.389 / 0.473 |
| C, on | 92 % | 44.4 | 5.848 / 6.365 / 6.698 | 0.421 / 0.483 |
| C, off | 93 % | 51.9 | 5.817 / 6.328 / 6.591 | 0.393 / 0.462 |
| C, off | 47 % | 60.1 | 5.782 / 6.327 / 6.532 | 0.239 / 0.344 |
| C, on | 79 % | 52.9 | 5.862 / 6.380 / 6.622 | 0.306 / 0.367 |

In set B, on minus off is −0.01 ms at the median and −0.02 ms at p95, with the off runs 0.003 and 0.085 ms apart. In set C it is +0.055 ms at the median and +0.045 ms at p95, with the off runs 0.035 and 0.001 ms apart. In set C the difference is larger than the off/off spread, but the off arms also ran under different loads (93 and 47 %). **Inconclusive: the two sets disagree in sign, and on and off span the same 5.78 to 5.88 ms median across the eight runs, so any cost to the encode stage is below about 0.05 ms.** A first set (A) ran under a full build gate that started 30 s into it. It is not counted, and it gave on 5.771 and 5.773 against off 5.803 and 5.843 ms.

## October 26: holding later compositions of a claimed frame (not kept)

After the picture change below, a 60 Hz game on a 60 fps VRR stream still repeated game frames on this PC (one run on October 26: age p95 4.33 ms, 8.0 % late claims, 23 repeats; October 24: 0.28 ms). In the bad runs the late claims came in streaks: in the run from that morning, 9 streaks of three or more consecutive late claims, the longest 30, claims about 10 ms behind the game and one period apart.

A simulation (`simulate_captures`: a 60 Hz game with 0.8 ms noise, a hitch every 2 s, a second composition 0.1 to 1 ms after each frame and sometimes a third 3 to 4 ms after it) reproduces that state. After a hitch the pacer claims a later composition of the picture it has just claimed. That claim is the repeat, and it spends the credit the game's next frame needs. While the capture mean reads faster than the stream, credit refills at exactly one claim per period, so the deficit lasts until the game leaves a gap: 246 of 1202 claims late, p99 11.2 ms. Refilling credit from the picture rate instead of the capture mean made it worse (rate 60.5, about 25 repeats, 38 % late claims), because more compositions were claimed. The change tried here treats a capture within a third of the picture interval of the claimed picture as another composition of it. That capture waits like an unchanged picture until the static repeat (`static_period`, 50 ms by default), unless a new picture replaces it. In the simulation late claims fell to 0 with no repeats. Replaying the five traced 60 Hz sessions and the 120 Hz traces from that day gave identical claims with and without the change: none of them was in the bad state.

Measured as in the section below. The base is rc.2 with the picture change's `butterpollo.exe` (`3712b0bca`); "hold" is the same package with the change on top. At 60 Hz there were two ABBA batches of four runs per build, in the orders B H H B B H H B and H B B H H B B H. Repeats are the client motion probe's count of game frames shown twice.

| Game / build | Runs | Age p50 / p95 / p99 | Present-to-sent p50 / p95 / p99 | Cadence error p50 / p99 | Stream fps | Late claims | Repeats per run |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 60 Hz, base | 8 | 0.25 / 0.87 / 3.10 | 3.40 / 5.27 / 7.37 | 0.43 / 5.68 | 59.84 | 1.3 % | 39 (20-79) |
| 60 Hz, hold | 8 | 0.25 / 0.97 / 3.30 | 3.56 / 5.89 / 8.11 | 0.53 / 6.09 | 59.84 | 1.1 % | 39 (22-66) |
| 120 Hz, base | 2 | 0.23 / 0.76 / 2.62 | 3.29 / 5.23 / 6.78 | 0.47 / 4.74 | 59.81 | 0.8 % | 0 |
| 120 Hz, hold | 2 | 0.23 / 0.29 / 3.53 | 3.16 / 4.91 / 7.17 | 0.53 / 4.92 | 59.74 | 1.1 % | 0 |
| 240 Hz, base | 1 | 0.25 / 1.46 / 3.74 | 3.79 / 6.50 / 9.11 | 1.25 / 6.81 | 59.80 | 1.3 % | 0 |
| 240 Hz, hold | 1 | 0.24 / 0.70 / 3.02 | 3.46 / 5.48 / 7.93 | 1.11 / 6.32 | 59.73 | 0.8 % | 0 |

None of the 16 runs at 60 Hz entered the streaking state: no streak was longer than two late claims, so the hardware could not show what the change targets. Age p95 per run: base 0.29 to 3.49 ms, hold 0.32 to 2.39 ms. Neither build came back near October 24's 0.28 ms, and both repeated a game frame about 39 times per run. Present-to-sent p95 was higher with the hold in most runs (per run 4.79 to 6.86 ms against 4.28 to 7.72 ms; rank test p ≈ 0.07), and so was the capture stage's median (0.37 against 0.20 ms). The 120 and 240 Hz games, two runs and one run per build, gave lower age p95 with the hold, but that is too few runs to count. **The change measured no better at 60 Hz and possibly worse at present-to-sent p95, so it is not kept.**

These runs had a different source of late claims: the 60 Hz game's presents landed on the display's 4.17 ms refreshes, and the prediction waited until the end of its budget for a next picture estimated from compositions. The repeats are still open.

## October 26: replying from the address the client reached (#54)

The host's UDP sockets answer each client from the local address its datagrams arrived on. When a peer's arrival address is new, and again at most once a second while its datagrams arrive (every 100 ms at most once a send to it failed as unreachable; the sending thread only flags it), the receiving thread asks Windows which address it routes that peer from (`SIO_ROUTING_INTERFACE_QUERY`). Only when that differs, as with a VPN such as Tailscale routing the LAN, are replies pinned with `IP_PKTINFO` or `IPV6_PKTINFO` through `WSASendMsg`; otherwise they go out with `sendto` as before, and while no peer is pinned a send does not take the table's lock.

`send_path_cost` (`rust/windows/src/net.rs`, `--release --ignored`) times the send path alone on loopback, four senders in one run: "before" makes the calls `848095f08` made (plain `sendto` and segmented batches, no table), "routed" has never heard from the peer, "matched" learned it with Windows' real route, which equals the arrival address, and "pinned" learned it with a faked route elsewhere. Each turn sends 200 segmented 64-datagram batches of 1200 B, then 2000 single 400 B datagrams; 40 rounds of one turn per sender in rotating order, a thread draining the receiver. The run ends by asserting no datagram was dropped and that only the pinned sender was pinned. Means of the 40 turns, averaged over four runs, the range of the four run means in brackets. Isolated from the installed service, no stream.

| Replies | Batch of 64 | Single datagram |
| --- | --- | --- |
| Before (`848095f08` calls) | 214.6 µs (208.4 to 228.2) | 10.74 µs (10.69 to 10.81) |
| Routed, peer not learned | 215.2 µs (209.6 to 227.8) | 10.81 µs (10.63 to 10.90) |
| Matched, not pinned | 215.6 µs (207.6 to 229.7) | 10.65 µs (10.41 to 11.03) |
| Pinned | 216.8 µs (208.7 to 231.1) | 11.31 µs (11.19 to 11.38) |

**A LAN client Windows already routes from the address it reached costs what it did before**, within the spread between runs, for batches and single datagrams. A pinned reply costs about 0.6 µs more CPU per single datagram (about 5 %) and no measurable difference per 64-datagram batch. The figures are higher than earlier single-mode runs (about 6.7 µs a datagram) because this PC's send cost roughly doubles a few seconds into the run on every path alike; the rotation spreads that over all four senders. An earlier version of this section put a pinned reply at twice `sendto`: it timed each mode in its own block with pinned always second, so it measured that slowdown, not the pin. Not measured: a real NIC, a VPN, and a stream's end-to-end latency.

`receive_path_cost` (same file, `--release --ignored`) times the receive side the control thread pays before it applies input, per 64 B datagram on loopback: "before" is `recv_from`, "settled" `WSARecvMsg` and `answer_from` for a peer checked within the last second, "checked" the same with Windows' route asked every time, as happens once a second per peer. 40 rounds of 500 datagrams per receiver in rotating order, the time from readiness to the datagram's handling; means of three runs on October 27, with a 4K120 stream to an Xbox running:

| Receive | Per datagram |
| --- | --- |
| Before (`recv_from`) | 0.71 µs (0.71 to 0.73) |
| Settled peer | 0.79 µs (0.78 to 0.79) |
| Route checked | 3.25 µs (3.22 to 3.28) |

**Input waits about 0.08 µs longer per datagram, and 2.5 µs once a second per peer.** The same Xbox stream's host stages matched 2.5.6 (present to send 7.04 ms mean, 8.16 ms p99, against 7.03 and 8.16), and its jitter showed in ICMP pings too (1 to 7 ms, the router under 1 to 3 ms), so it is the path to the client, not the host.

## October 26: VRR claims follow the game's pictures

A 120 Hz game on a 60 fps VRR stream kept an age p95 of 3.7 ms on [October 24](#october-24-predictive-claim-waits-under-vrr): some claims still took a picture close to a game frame old. The pacing traces (`--record`) show why. WGC on this PC's 240 Hz VRR display delivers each frame of a 120 Hz game twice, the second composition 0.1 to 1 ms after the first, and in some sessions a third time 3 to 4 ms later. The median of the capture intervals then reads about 7 ms, or 3 ms with the third composition, for a game that presents every 8.3 ms. A claim predicted the game's next frame too early, stopped waiting at the end of its budget, and took the latest composition, 7 to 8 ms old, just before the game's next frame arrived. The claim-wait commit `abb803c67` and 3.0.0-rc.2 measure the same today (age p95 7.67 and 7.27 ms, two runs each), so the share of late claims depends on the capture pattern, not on a commit.

The capture worker now also counts pictures: a capture less than 2 ms after the previous one belongs to the same picture. Claims predict the next picture from the first capture of the latest one with the median picture interval, and wait until the second-longest of the last 16 picture intervals, at most a quarter period past the median, plus 1 ms to notice it (`Freshness::picture_interval`, `Pacer::observe_picture`). A source without paired compositions has the same median as before, and a 1000 Hz display has no picture interval and no prediction, as before. Unit tests: `a_second_composition_of_a_picture_does_not_shorten_the_picture_interval`, `a_second_composition_of_each_game_frame_does_not_age_vrr_claims` (late claims under 0.5 % with second and third compositions, 10 % and 52 to 57 % from the capture median) and `a_picture_later_than_the_median_is_waited_for_within_the_late_interval`. Replaying the traced capture timestamps through both pacers: late claims 7.3 % to 0.0 % in the rc.2 session, 45 % to 0.3 % in a three-composition session.

Measured on the RX 7900 XT as on October 24: HEVC 2560x720, VRR hint on, `minimum_fps_target=20`, `motion_probe` on the 240 Hz display, timing record on, 25 s per run, the first 5 s `stream timings` line dropped, means of the remaining lines and of the runs. rc.2 is the release package (`56dffeeb4`); "pictures" is the same package with this change's `butterpollo.exe`. Isolated test hosts on their own port, the installed service idle and untouched, no encoder hang. Late claims are claims taken more than 4 ms after their picture was published (claim trace).

| Game / build | Runs | Age p50 / p95 / p99 | Present-to-sent p50 / p95 / p99 | Cadence error p50 / p99 | Stream fps | Late claims |
| --- | --- | --- | --- | --- | --- | --- |
| 120 Hz, rc.2 | 4 | 0.25 / 6.61 / 8.06 | 3.40 / 8.79 / 10.33 | 0.28 / 8.07 | 59.82 | 7.7 % |
| 120 Hz, pictures | 4 | 0.24 / **2.34** / **3.37** | 2.90 / **4.71** / **5.69** | 0.27 / **3.43** | 59.85 | 0.1 % |
| October 24, B | 2 | 0.27 / 3.71 / 8.91 | 3.80 / 6.39 / 11.43 | 0.24 / 8.83 | 60.10 shown | |
| 240 Hz, rc.2 | 1 | 0.21 / 0.28 / 0.30 | 2.44 / 2.81 / 3.15 | 0.22 / 0.92 | 59.92 | 0.0 % |
| 240 Hz, pictures | 1 | 0.21 / 0.26 / 0.28 | 2.51 / 2.83 / 3.04 | 0.19 / 0.95 | 59.77 | 0.1 % |
| 60 Hz, rc.2 | 1 | 0.22 / 8.48 / 11.89 | 4.25 / 10.97 / 14.40 | 0.39 / 12.62 | 59.77 | 22.7 % |
| 60 Hz, pictures | 1 | 0.22 / 4.33 / 11.20 | 3.41 / 6.70 / 13.59 | 0.22 / 10.57 | 59.74 | 8.0 % |

Order at 120 Hz: rc.2, pictures, pictures, rc.2, rc.2, pictures, pictures, rc.2. **The 120 Hz game's age p95 falls from 6.61 to 2.34 ms and p99 from 8.06 to 3.37 ms, present-to-sent p95 from 8.79 to 4.71 ms and the cadence error p99 from 8.07 to 3.43 ms**, with the medians and the frame rate unchanged; late claims fall from 7.7 % to 0.1 %. Every pictures run is below October 24's 3.71 ms p95 (per run 2.72, 1.67, 2.86 and 2.12 ms); rc.2's runs gave 7.49, 7.42, 7.69 and 3.84 ms, the last in a session with few second compositions. The 240 Hz game, one run per build, measured the same (age p95 0.28 and 0.26 ms; October 24: 0.30). The 60 Hz game was not clean on either build today, unlike October 24 (age p95 0.28 ms): in one run each the stream carried the same game frame twice 68 times on rc.2 and 23 times with pictures, with late claims of 22.7 and 8.0 % and age p95 8.48 and 4.33 ms. Pictures is better there too; what still makes a 60 Hz game's claims late on this PC is not found. Kept.

## October 26: 3.0.0-rc.2 against the published numbers

The 3.0.0-rc.2 release package (`v3.0.0-rc.2`, `56dffeeb4`; PyroWave with a fixed host, see below) on the RX 7900 XT (5120x1440 240 Hz display), each case run the way the published number was measured. Isolated test hosts on their own profiles and ports; the installed service (also rc.2) was idle before every run and never touched. One stream at a time, no encoder hang (no exit code 3).

**Timing record, loopback 1080p60, default bitrate** (as [October 23](#october-23-per-frame-timing-record-baselines): `e2e.py --record`, 60 s per run, `shared_clock` join with bound 0.0 ms, every host row matched a client row). Each figure is the median over the runs of each run's median / p95 / p99, in ms. The rc.2 records stamp the game's own Present call through ETW (`present_source=etw`), so their `end_to_end` starts about 2.09 ms earlier than the October 23 records, which started at the capture's present stamp (`game_to_present` is 2.089 / 2.094 / 2.094 ms in every run). The like-for-like row subtracts each frame's `game_to_present`.

| Stage | H.264, 3 runs (Oct 23: 2) | HEVC, 3 runs (Oct 23: 2) | AV1, 2 runs (Oct 23: 3) | PyroWave, 3 runs (Oct 23: 3) |
| --- | --- | --- | --- | --- |
| Encode | 2.49 / 2.97 / 3.36 (2.59 / 3.35 / 3.80) | 2.42 / 2.88 / 3.09 (2.56 / 3.38 / 3.85) | 2.26 / 2.63 / 2.84 (2.28 / 2.85 / 3.28) | 0.78 / 0.99 / 1.10 (0.93 / 1.39 / 1.55) |
| Host total (present to last sent) | 3.03 / 10.52 / 11.42 (3.67 / 10.62 / 11.64) | 2.90 / 8.89 / 11.29 (3.63 / 10.54 / 11.65) | 2.70 / 3.18 / 9.68 (4.24 / 10.85 / 11.67) | 9.89 / 10.33 / 17.32 (11.36 / 18.02 / 18.90) |
| Decode | 3.72 / 4.48 / 4.65 (3.54 / 4.75 / 5.33) | 5.12 / 6.69 / 7.18 (5.02 / 6.96 / 8.12) | 3.17 / 4.23 / 4.99 (4.36 / 6.51 / 8.11) | 1.22 / 1.44 / 1.55 (1.85 / 2.58 / 3.19) |
| End to end from the capture present, as Oct 23 | **6.92 / 13.97 / 15.66** (7.41 / 14.04 / 15.73) | **8.37 / 14.01 / 17.50** (8.81 / 15.64 / 17.78) | **5.89 / 7.21 / 12.90** (8.82 / 15.68 / 17.70) | **11.14 / 11.68 / 18.57** (13.34 / 20.01 / 20.87) |
| End to end from the game's Present (new) | 9.01 / 16.05 / 17.74 | 10.46 / 16.10 / 19.56 | 7.98 / 9.30 / 14.96 | 13.23 / 13.77 / 20.66 |

- **Median end to end is 0.5 ms lower on H.264, 0.4 ms on HEVC and 2.9 ms on AV1.** The host total is 0.6 to 1.5 ms lower at the median, and AV1's decode on the receiver 1.2 ms lower. H.264 and HEVC p95 and p99 are equal within the run spread; AV1's p95 fell from 15.68 to 7.21 ms.
- **The runs disagree at p95.** The host total is bimodal: in some runs more than 5 % of frames wait close to a frame period between present and send, in others almost none (H.264 per-run medians 6.06, 7.40, 6.92 ms and p95 6.83, 14.84, 13.97; HEVC medians 7.06, 9.54, 8.37). Two H.264 and two HEVC runs disagreed, so each got a third. The two AV1 runs agreed (medians 5.82 and 5.96) and both landed in the low mode, so AV1's p95 gain is not claimed beyond these two runs.
- **PyroWave median end to end is 2.2 ms lower and p95 8.3 ms lower.** Encode is 0.15 ms lower at the median and decode on the receiver 0.63 ms. Sending still spreads each frame over about 9 ms (8.90 / 9.20 / 9.48, against 8.87 / 9.28 / 9.58), and that pacing is most of the host total. The p99 is a tail: 0.36, 1.2 and 4.0 % of frames in the three runs waited about a frame period between present and send (host total p99 10.66, 17.32 and 18.12 ms), while p95 agreed across runs (10.29 to 10.39 ms). All three runs decoded and matched all 3592 frames at 60.0 to 60.02 fps. Each was granted 18.7 Mbit/s of the default 20 Mbit/s, and the host warned that this bitrate is too low for PyroWave.
- **The PyroWave runs used a fixed host.** rc.2 itself (and rc.3) cannot offer PyroWave from a host started without administrator rights, which is how `e2e.py` starts it: the first two runs and a rerun of both failed that way (02:58 to 03:06 local). The isolated host's optional PyroWave probe timed out after 15 s (`optional PyroWave probe failed ... optional codec probe timed out`), the host did not offer PyroWave, and `e2e.py` stopped before streaming. Such a host started its programs with the system's environment (`CreateEnvironmentBlock` without a token): `USERPROFILE` was `C:\Users\Default`, `USERNAME` SYSTEM, `TEMP` `C:\Windows\Temp`. The AMD driver keeps its D3D12 shader cache under the profile, and the one under `C:\Users\Default` is stale and read-only. So the probe worker compiled all four PyroWave modes' pipelines from scratch: 24 s and 67 s of CPU, against 1.4 s with the user's profile. Setting only `USERPROFILE=C:\Users\Default` reproduced it, and `TEMP` or `USERNAME` alone did not. A cold but writable cache, as after an install or a driver update, took 7.1 s (18.6 s of CPU), then 1.25 s, so the 15 s limit stays. The rc.2 release check's PyroWave case at 01:38 probed in 2.9 s, most likely from an elevated shell, whose host starts programs with the desktop's token and the user's environment. These three runs (10:46 to 10:56 local) used the rc.2 package with `butterpollo.exe` built from rc.2, the ENet loss fix `2735ba786` and the environment fix `2a058c4e5`. Neither fix touches the frame path.

**VRR, 120 Hz game on a 60 fps stream** (as [October 24](#october-24-predictive-claim-waits-under-vrr): HEVC 2560x720, VRR hint on, `minimum_fps_target=20`, `motion_probe` at 120 Hz, timing record on for the cadence error, 25 s per run, the first 5 s `stream timings` line dropped, means of the remaining lines and of the runs). Previous is leg B of October 24.

| | Age p50 / p95 / p99 | Present-to-sent p50 / p95 / p99 | Cadence error p50 / p99 | Shown fps |
| --- | --- | --- | --- | --- |
| rc.2, 2 runs | 0.25 / 6.24 / 8.35 | 3.50 / 8.62 / 10.79 | 0.37 / 8.58 | 60.10 |
| October 24, B | 0.27 / 3.71 / 8.91 | 3.80 / 6.39 / 11.43 | 0.24 / 8.83 | 60.10 |

p50 and p99 are equal or slightly lower; **p95 is worse** (age 6.24 against 3.71 ms, present-to-sent 8.62 against 6.39 ms). Four more runs without the timing record agree (age 0.26 / 7.65 / 8.25, present-to-sent 3.56 / 9.79 / 10.65), so it is not run-to-run noise. The claim-wait commit `abb803c67` measures the same today, so no commit caused it: the capture pattern on this PC did, and [VRR claims now follow the game's pictures](#october-26-vrr-claims-follow-the-games-pictures). October 24 kept two `stream timings` lines a run, these runs three, so the p95 columns are noisy on both sides.

**The laptop over Wi-Fi** (`smoke.ps1`, as in the release checks: Moonlight-qt 6.2.0 on the Radeon 780M laptop, 5 GHz Wi-Fi at 2402 Mbit/s, client "lap" on its Extended virtual display against the installed rc.2 host, `motion_probe` at 240 Hz on the streamed display, 1968x2184 HDR 120 fps 80 Mbps, 30 s per codec, Moonlight's whole-session averages). Previous: the October 8 rc.24 runs, one 35 s run each ([PERFORMANCE_WORK.md](PERFORMANCE_WORK.md#first-remote-client-the-laptop-over-wi-fi)).

| Codec | Received fps, run 1 / run 2 (Oct 8) | Host processing avg, run 1 / run 2 (Oct 8) | Decode avg | Jitter drops |
| --- | --- | --- | --- | --- |
| AV1 | 115.4 / 120.1 (121.2) | 0.7 / 0.5 ms (2.9) | 0.54 / 0.34 ms | 0.11 / 0.10 % |
| HEVC | 119.9 / 120.2 (121.0) | 0.6 / 0.6 ms (3.5) | 0.33 / 0.54 ms | 0.10 / 0.10 % |

Both runs passed. AV1's 115.4 fps in run 1 is Moonlight's session average, start included; the host's own 5 s lines in that session show 119.7 to 119.9 fps throughout, and run 2 got 120.1. Moonlight's host processing field dropped from about 3 ms to 0.5 to 0.7 ms; the host's present-to-sent p50 in run 1 was 3.5 ms (AV1) and 4.1 ms (HEVC), so the field's lower value is not claimed as a latency gain.

**Not rerun.**

- **Link adaptation, `r50-hevc-hi`** (previous 1.17 and 1.08 % frames lost): `linkspeed.py` lets the NUC client in through a firewall rule it adds for the package's exe, and this shell was not elevated, so the rule was not created and the client could not reach the host (`ConnectTimeout` on `/applist`). The harness then picked up client files left in the container from an earlier run; both rows were rejected (`no host record`). `linkspeed.py` now empties the client's folder before each arm and stops at once without an elevated shell.
- **WebRTC glass to glass** (H.264 and AV1 1080p60, HEVC and AV1 4K120): `webrtc_e2e.cjs` needs its elevated helper, which is not installed on this PC (`rust/release/elevation.ps1` from an elevated shell sets it up).
- **Picture age under GPU load** (2.2.9: HEVC 14.34 / 16.03, AV1 13.65 / 18.00): the October 12 harness (native HDR virtual display as SYSTEM) is not in the repository, and SYSTEM runs need the same elevated helper.
- **Real-client picture age over Wi-Fi** (October 8: 13.5 ms render to received): it used a laptop-side decoder with a UDP clock map that is not in the repository.

Raw runs are under `C:\src\butterpollo-release\perf-rc2` (`timing`, `vrr`, `laptop`, `link`; the failed PyroWave runs under `timing\failed` and `timing\e2e-pyrowave-*`).

## October 25: WebRTC input delay on the 780M laptop

Question: how much time does the host add between the browser's input frame and `SendInput`? Host: a release build of the WebRTC server on the Ryzen 7 8845HS laptop (isolated profile, injection into an echo window there; the browser runs on the dev PC). The leg injects 29 checked sequences (keys, text, relative and absolute moves, buttons, wheel, stuck keys, gamepad mouse mode) and the host logs one "WebRTC input summary" per stream.

| Part | Before the fix | After |
| --- | --- | --- |
| Queue delay, receipt to the injector call, max | 25.3 to 26.1 ms (this included the injector call) | 0.11 to 0.52 ms |
| Channel hop (`wait_max_us`) | not measured | 49 to 512 us |
| Pointer refresh (`refresh_max_us`) | not measured | 28 to 48 us |
| `SendInput` (`apply_max_us`), echo window on low-level hooks | not measured | 7.2, 9.3, 8.5 ms |
| `SendInput` (`apply_max_us`), echo window on Raw Input | not measured | 28.1, 36.3, 37.6 ms |
| Bare `SendInput` probe, no host, first left press after about 4 s idle | not measured | 24 to 27 ms |

The "before" queue figure was the injector call counted as queue time; the worker now stamps the delay where it calls the injector, so it is the host's own wait. The stall is the first left-button press after a few idle seconds (every other event is under 1 ms) and is the same with the host out of the picture, with either way of reading the input in the echo window. The likely cause is Windows handling a click on the window under the cursor; not isolated further. ENet's control thread makes the same call. Three runs per echo mode, one laptop, one Wi-Fi link. Outputs: `C:\tests\webrtc-laptop\b5k\` on the dev PC.

## October 25: Wi-Fi pacing hint on a real laptop

Question: does pacing a Wi-Fi client from the first frame (`x-rl-video[0].clientWireless:1`, `wireless_adaptive_pacing`) cut loss without adding latency? Host: this PC (RX 7900 XT, wired 2.5 GbE), a release build of main at the pacing change, isolated profile, WGC capture with a full-screen scrolling texture at 60 fps. Client: Nightjar desktop on the Ryzen 7 8845HS laptop, Wi-Fi only. Arm A is Nightjar 3.1.0, which sends no hint, so the host starts at 800 Mbps route pacing; arm B is the new client, so the host starts adaptive pacing at 94 Mbps. Both arms use the same FFmpeg and SDL DLLs. HEVC 1080p60, the host capped the bitrate at 46,988 kbps, adaptive bitrate off, 90 s per run in the order A B A B A B, loss memory cleared before every run.

| Link | Arm | FEC-unrecoverable frames (mean) | IDR requests after the first (mean) | Bitrate step-downs (mean) | End-to-end median / p99 (mean of runs) | Frames decoded of about 5400 (mean) |
| --- | --- | --- | --- | --- | --- | --- |
| 5 GHz, 2402 Mbit/s, signal 95-96% | A, no hint | 0 | 0 | 0 | 4.74 / 10.12 ms | 5397 |
| 5 GHz, 2402 Mbit/s, signal 95-96% | B, hint | 0 | 0 | 0 | 4.77 / 10.18 ms | 5399 |
| 2.4 GHz ch 13, 287 Mbit/s, signal 80-85% | A, no hint | 61 (13-115) | 100 (17-195) | 3.7 | 64 / 12,170 ms | 3925 |
| 2.4 GHz ch 13, 287 Mbit/s, signal 80-85% | B, hint | 15.7 (4-35) | 27.7 (9-54) | 1.7 | 13.7 / 1652 ms | 4728 |

On the clean 5 GHz link the hint changes nothing measurable: no loss in either arm. B's host send stage is 5-8 ms at p99 because each frame is spread out, but that does not show up end to end. On 2.4 GHz every run lost video, and B lost about a quarter as many unrecoverable frames and needed about a quarter as many keyframes, with a much shorter latency tail. Three runs per arm with a wide spread inside each arm, so the direction is solid and the ratios are not. B still stalls after loss on this link, so 47 Mbps HEVC remains too much for it. One laptop, one AP; phones not measured. Outputs: `C:\src\butterpollo-release\wifi-ab\` and `wifi-ab-24\` on the test PC.

## October 25: WebRTC on the real host, pacing, RTX, 90 and 120 fps, 4K and HDR (B5b)

The browser transport (`docs/webrtc-design.md`) measured against the real host with real browsers: send pacing on and off, NACK with RTX at 0.1 % datagram loss, 1080p at 90 and 120 fps, 4K with a keyframe every 500 ms, and HDR at 4K60 from the virtual display. Three decisions follow from the numbers: send pacing stays on, RTX and NACK are now in the default offer, and 90 and 120 fps are offered at every mode the encoder's throughput allows.

**Fixture.** This PC: RX 7900 XT (AMF), Windows 11, physical monitor 5120x1440 at 240 Hz as the primary display, release build of branch `wr-b5h` (the host with the switches below). Test host on an isolated profile (`butterpollo.exe --config-dir C:\tests\webrtc --port 48140 --bind 127.0.0.1`, WebRTC media on 48545/48546), `virtual_display_layout = extended` throughout. The client is `rust/tests/webrtc_e2e.cjs` driving our protocol v2 harness client (`rust/tests/webrtc_client/client.js`) with Playwright: Microsoft Edge 155.0.4283.62 for H.264 and AV1, Chrome 155.0.8059.85 with `WebRtcAllowH265Receive` for HEVC. The browser runs on the same PC over loopback, so these runs show the host and the transport, not a network. Content is the harness clock window (a magenta-bordered clock on the physical desktop) plus a tone; 1080p60 at the default 20 Mbit/s, 4K at 50 Mbit/s requested. The desktop is nearly static, so the encoders send far less than the requested rate (H.264 3 to 4 Mbit/s, AV1 0.3 Mbit/s at 1080p60); every number below is for that content.

**Method.** One session per leg, 40 s each. Glass-to-glass (g2g) is the harness probe: it reads the clock window out of the decoded video in the browser and subtracts it from the clock on the physical screen. Loss, freezes, key frames, NACK and PLI come from the browser's `getStats`. Host fps, encode time and present-to-send come from the host's 5 s `stream timings` lines. A/B arms alternate (A, B, A, B, A, B) so drift hits both. Switches (hidden config keys, added in this branch): `webrtc_pacing = false` turns the send pacer off, `webrtc_frame_rates = 30,60,90,120` replaces the offered rates, `webrtc_rtx = false` takes NACK and RTX out of the offer (they are in it by default), `webrtc_loss_permille = 1` drops 0.1 % of the video datagrams at the media socket (fixed seed, video only). The HDR and virtual display legs need the virtual display driver, so that host runs as SYSTEM in the console session (`rust/windows/examples/system_launch.rs` through a one-shot task) with its own profile `C:\tests\webrtc-system`. The physical monitor's mode is never touched.

### Send pacing on and off (3 pairs, 40 s)

g2g median / p95 / p99 in ms, mean of pairs 1 and 2 (loss 0, NACK 0 and PLI 0 in every run; no freeze in any run of these two pairs):

| Leg | Paced (default) | Unpaced |
| --- | --- | --- |
| H.264 1080p60 | 21.0 / 24.0 / 32.5 | 21.0 / 24.0 / 27.0 |
| AV1 1080p60 | 17.0 / 19.5 / 21.5 | 17.5 / 21.5 / 27.5 |
| HEVC 4K60, keyframe every 500 ms | 29.5 / 39.0 / 44.0 | 33.0 / 46.0 / 52.5 |
| AV1 4K60, keyframe every 500 ms | 28.0 / 37.0 / 40.0 | 34.5 / 51.0 / 56.0 |

p95 for each of the three pairs (paced, then unpaced): H.264 22, 26, 72 and 22, 26, 62; AV1 18, 21, 80 and 18, 25, 63; HEVC 4K 40, 38, 72 and 41, 51, 83; AV1 4K 35, 39, 78 and 40, 62, 79.

- Pair 3 is contaminated: cargo builds for the next change ran on this PC during it, and both arms show medians of 50 to 60 ms. It is reported, not used for the decision. In it the unpaced arm was better at 1080p (p95 62 against 72 for H.264, 63 against 80 for AV1; the paced AV1 run also had one 0.2 s freeze) and equal at 4K.
- In the clean pairs unpaced is never better outside the noise, except H.264 p99 (27.0 against 32.5 ms). At 4K with a keyframe every 500 ms the pacer takes 7 to 14 ms off p95 (HEVC 46 to 39, AV1 51 to 37), consistent with it spreading a keyframe's packets over the frame interval instead of sending them as one burst. Key frames per run were 74 to 81 either way; mean encode 6.1 ms (HEVC) and 5.5 ms (AV1).
- Verdict: keep the pacer on. No default change.

### RTX and NACK at 0.1 % loss (3 pairs, 40 s, `webrtc_loss_permille = 1`)

Arm A is loss with `webrtc_rtx = false`, the old default: a lost packet is recovered by a PLI and a keyframe. Arm B is loss with NACK and RTX, which is now the default.

| Leg, arm | Decoded fps | Packets lost | Freezes (seconds frozen) | PLI | NACK | g2g median / p95 (ms) |
| --- | --- | --- | --- | --- | --- | --- |
| H.264 1080p60, A, runs 1 / 2 / 3 | 42.1 / 42.3 / 36.5 | 8 / 8 / 13 | 3 (12.1) / 3 (12.1) / 6 (15.7) | 4 / 4 / 6 | 0 | 38 / 48, 21 / 25, no samples |
| H.264 1080p60, B, runs 1 / 2 / 3 | 60.2 / 60.1 / 60.2 | 13 / 13 / 13 | 0 / 0 / 0 | 0 | 13 / 13 / 13 | 17 / 25, 17 / 21, 21 / 25 |
| AV1 4K60, A, runs 1 / 2 / 3 | 42.1 / 43.4 / 43.4 | 28 / 27 / 29 | 17 (14.5) / 15 (13.3) / 18 (10.7) | 1 / 1 / 0 | 0 | 56 / 73, 31 / 42, no samples |
| AV1 4K60, B, runs 1 / 2 / 3 | 60.2 / 60.5 / 60.2 | 28 / 32 / 26 | 0 / 0 / 0 | 0 | 28 / 32 / 26 | 29 / 39, no samples, 26 / 35 |

- With RTX every lost packet was NACKed and resent: NACK equals packets lost in all six runs, no PLI, 60 fps decoded, no freeze. Without it the same 0.1 % loss cost 28 to 39 % of the decoded frames and 11 to 16 s of frozen picture in 40 s.
- Three runs have no g2g samples (the probe lost the clock window; fps, loss and freezes of those runs are still valid). Where both arms have samples, p95 is 25 to 73 ms without RTX and 21 to 39 ms with it.
- Clean link (3 pairs, no loss): RTX changes nothing. H.264 g2g median / p95 / p99 19.7 / 26.0 / 31.3 ms without and 18.7 / 24.7 / 34.0 ms with; AV1 4K 29.7 / 39.3 / 43.0 without and 30.3 / 37.3 / 40.0 with. Loss, NACK, PLI and freezes were 0 in all 12 runs, and the sent Mbit/s were within 1 of each other.
- Verdict: RTX and NACK are on by default (`webrtc_rtx = false` turns them off). The design doc said the better arm ships.

### 90 and 120 fps at 1080p (3 runs per leg, 40 s)

`webrtc_frame_rates = 30,60,90,120` on the test host. Decoded fps is the browser's; host fps is the range of the 5 s timing means. Loss, freezes, NACK, PLI and key frames were 0 in all 18 runs.

| Leg | Decoded fps | Host fps | g2g median / p95 / p99 (ms), runs 1 / 2 / 3 | Mbit/s |
| --- | --- | --- | --- | --- |
| H.264 90 | 90.1 / 90.2 / 90.2 | 89.7 to 89.9 | 16 / 21 / 21, 16 / 21 / 25, 16 / 21 / 25 | 5.5 to 5.9 |
| H.264 120 | 120.3 / 120.2 / 120.2 | 119.8 to 119.9 | 18 / 24 / 26, 16 / 20 / 21, 21 / 25 / 27 | 7.9 to 8.7 |
| HEVC 90 | 90.2 / 90.2 / 90.2 | 89.9 to 90.0 | 19 / 25 / 29, 19 / 26 / 31, 16 / 23 / 28 | 2.7 to 3.1 |
| HEVC 120 | 120.3 / 120.3 / 120.2 | 119.9 to 120.0 | 17 / 24 / 26, 17 / 22 / 26, 17 / 25 / 30 | 3.6 to 3.7 |
| AV1 90 | 90.1 / 90.2 / 90.1 | 89.8 | 15 / 21 / 22, 16 / 21 / 22, 20 / 25 / 26 | 0.5 to 1.0 |
| AV1 120 | 120.3 / 120.3 / 120.4 | 119.9 to 120.1 | 24 / 28 / 30, 17 / 23 / 26, 17 / 23 / 26 | 0.4 to 0.5 |

- Encode time stayed 2.5 to 2.7 ms and present-to-send p99 7.0 to 9.4 ms at 90 and 120 fps. At 60 fps the same fixture gave g2g median / p95 of 21.0 / 24.0 (H.264) and 17.0 / 19.5 (AV1) in the clean pacing pairs, so 90 and 120 fps are not slower end to end.
- Verdict: 90 and 120 fps measured clean at 1080p with all three codecs.

### 90 and 120 fps at every mode, gated by the AMF throughput (1 run per leg, 40 s)

The default offer is 30, 60, 90 and 120 fps at every mode. The host reads the throughput caps of its H.264, HEVC and AV1 encoders at startup (16x16 macroblocks per second) and lists and accepts only the rates they take: on this RX 7900 XT they report 6,125,260 (H.264), 6,099,792 (HEVC) and 8,264,788 (AV1), against 3,888,000 needed for 4K120, so nothing is dropped here; a lower-throughput GPU drops what it cannot do, and a request beyond the cap is refused with the codec, size and rate. One run each on the physical monitor (the default offer, no `webrtc_frame_rates`); loss, freezes, NACK, PLI and key frames were 0.

| Leg | Decoded fps | Host fps | g2g median / p95 / p99 (ms) | Encode (ms) | Mbit/s |
| --- | --- | --- | --- | --- | --- |
| HEVC 1440p120 | 120.3 | 119.9 | 23 / 31 / 38 | 3.3 | 5.7 |
| HEVC 4K120 | 120.3 | 120.0 | 25 / 32 / 36 | 6.1 | 13.0 |
| AV1 4K120 | 120.4 | 119.9 | 28 / 36 / 40 | 5.8 | 6.3 |

- H.264 is not offered at 4K, where 4K120 would need level 6.0, which browsers refuse. H.264 1440p120 was not measured. 720p at 90 and 120 was not measured either.
- The signalled levels hold every offered mode: HEVC 5.2 and AV1 level 5.2 (seq_level_idx 14, which has 5.3's limits) at 4K120, checked by a unit test across all modes and rates.

### 4K60 HDR from the virtual display (3 runs per codec, 40 s)

Virtual display in HDR (`virtual_display_mode = per_client`, `dd_hdr_option = auto`), host as SYSTEM, `hdr: true` at 3840x2160. The first 40 encoded frames were captured in the browser and run through ffprobe. The monitor count was 1 before and after every run, so the virtual display was removed.

| Leg | Decoded fps | Host fps | Freezes (seconds) | PLI | Mbit/s | Decode ms | Encode ms | Bitstream |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| HEVC Main10 (Chrome), runs 1 / 2 / 3 | 58.1 / 57.9 / 58.4 | 58.4 / 58.4 / 58.5 | 1 (0.54) / 1 (0.63) / 1 (0.49) | 0 | 1.1 | 0.96 / 1.11 / 0.88 | 6.1 to 6.2 | Main 10, yuv420p10le, bt2020nc, smpte2084, bt2020 primaries, tv range, 3840x2160 |
| AV1 10-bit (Edge), runs 1 / 2 / 3 | 57.8 / 58.2 / 57.9 | 58.4 / 58.5 / 58.4 | 1 (0.60) / 1 (0.52) / 1 (0.61) | 3 / 2 / 3 | 0.1 to 0.2 | 0.66 / 0.64 / 0.50 | 5.6 | Main, yuv420p10le, bt2020nc, smpte2084, bt2020 primaries, tv range, 3840x2160 |

- All six runs passed: the browser decoded 4K frames, `getStats` reported `video/H265` and `video/AV1`, the stream configuration said `hdr true, ten_bit true` with AMF and an `RgbaF16` source, and the bitstream carried mastering display metadata. Max CLL and max FALL are 0 from a virtual display, so only the mastering display metadata is checked.
- The same 4K AV1 on the virtual display in SDR (one run): 58.8 fps, no freeze, host 58.4 fps, encode 5.4 ms. The half-second freeze at the start was seen only in the HDR runs.
- `hdr: true` with the virtual display forced to SDR (`dd_hdr_request_override = force_off`) is refused with `stream_failure` and the text "HDR was requested but the display is not in HDR". Nothing is streamed.

### Caveats

- Loopback, one PC, one nearly static desktop. Bitrates are low (see Fixture); 90 and 120 fps with full-screen motion, and 4K at the full requested bitrate, were not measured. The 4K legs ask for a keyframe every 500 ms, which is the worst case for the pacer, not a typical stream.
- The HDR legs have no g2g: the clock window sits on the physical monitor, not on the virtual display. Every HDR run shows one freeze of about half a second near the start, and the host delivered 58.4 fps rather than 60 on the virtual display with this content; neither is explained.
- AV1 at 1080p is padded by the AMD encoder to 1920x1082; the harness accepts that size for AV1 only.
- Pair 3 of the pacing A/B ran with builds on the same PC (see above). Three RTX runs have no g2g samples. A Windows Firewall prompt for the first launch of a new test binary dims the clock and gives 0 samples, so the harness adds an inbound allow rule for its staged host.
- `webrtc_loss_permille` drops datagrams at the host's socket, so it models loss without added delay; the RTX result is for 0.1 % random loss only, not bursts and not a long round trip.
- Edge and Chrome are both 155 builds on this PC; no other browser, phone or Nightjar was measured here.

### Verdict

- Send pacing: kept on. It costs no latency at 1080p and takes 7 to 14 ms off p95 at 4K with frequent keyframes.
- RTX and NACK: on by default. At 0.1 % loss it kept 60 fps and no freeze where PLI recovery alone gave 36 to 43 fps and 3 to 18 freezes in 40 s, and on a clean link it changed nothing.
- Frame rates: every mode offers 30, 60, 90 and 120 fps, each gated by the encoder's AMF throughput. Measured clean here at 1080p (all three codecs), 1440p120 (HEVC) and 4K120 (HEVC, AV1); 720p and H.264 at 1440p above 60 were not measured.
- HDR at 4K60 works with HEVC Main10 and AV1 10-bit, with BT.2020, PQ and mastering display metadata in the bitstream, and is refused when the display is SDR.

## October 23: behaviour across client link speeds

Question: what does a stream look like when the client's link is slower than the PC's, and what breaks? Host: this PC, release package 3.0.0-beta.1 (AMF, WGC capture on the physical display, 1080p60, noise content so the encoder reaches its target bitrate), isolated profile, `pacing_max_bitrate_kbps` unset, every adaptive option at its default. Client: `nightjar-cli` (Nightjar `protocol/`, the Rust engine) in a container on the NUC (Debian 13, 1 GbE, VAAPI decode). The link is shaped on the container's receive side only (`ifb` + `tc tbf`, 50 ms drop-tail queue unless stated, [linkshape.sh](release/linkshape.sh)); each rate was checked with `iperf3 -u` before use. 45 s per arm, first 5 s discarded, loss memory reset before every arm ([linkspeed.py](release/linkspeed.py), [linkspeed_client.py](release/linkspeed_client.py)). Not moonlight-qt and not a phone: decode times and the Android client's own ABR are not represented.

Picture age is the A8 `end_to_end` (host present to client shown, clock-synced). Loss is the share of frames the host sent after warm-up that the client never showed. A freeze is a gap above 100 ms between shown frames. Cells are p50 / p99 ms, loss %, freezes (count / total). "none" means the client showed no frame after warm-up.

| Link Mbit/s | HEVC 20 Mbps | AV1 20 Mbps | H.264 20 Mbps | HEVC above the link (Mbps) |
| --- | --- | --- | --- | --- |
| unshaped (1 GbE) | 12.9 / 26.1, 0, 0 | 4.9 / 8.4, 0, 0 | 12.9 / 26.7, 0, 0 | 300: 27.7 / 138.5, 1.5, 1 / 0.3 s |
| 500 | 13.1 / 26.5, 0, 0 | 5.0 / 8.5, 0, 0 | 12.7 / 25.7, 0, 0 | 500: 30.6 / 180.5, 7.1, 4 / 1.8 s |
| 300 | 12.9 / 25.7, 0, 0 | 5.1 / 6.8, 0, 0 | 13.4 / 24.5, 0, 0 | 450: 161 / 290, 96.7, 13 / 31.9 s |
| 150 | 12.9 / 25.7, 0, 0 | 5.1 / 7.7, 0, 0 | 12.6 / 25.9, 0, 0 | 225: 188 / 292, 99.3, 6 / 22.4 s |
| 80 | 14.2 / 27.0, 0, 0 | 6.0 / 8.5, 0, 0 | 14.0 / 27.5, 0, 0 | 120: none, 100 |
| 50 | 16.6 / 28.0, 0, 0 | 8.0 / 20.5, 0, 0 | 16.5 / 30.7, 0, 0 | 75: none, 100 |
| 30 | 20.9 / 34.5, 0, 0 | 11.7 / 23.8, 0, 0 | 20.6 / 34.6, 0, 0 | 45: none, 100 |
| 20 | 58.8 / 83.2, 15.1, 21 / 6.7 s | 56.7 / 72.1, 11.9, 15 / 4.9 s | 54.4 / 86.8, 36.2, 63 / 14.9 s | 40: none, 100 |
| 10 | no frame joined | none, 100 | none, 100 | 40: no frame joined |

"Above the link" is 1.5x the rate, at least 40 Mbps, at most 500. 4K60 HEVC at 80 Mbps: 300 Mbit/s link 21.9 / 37.0, 0 % lost; 150 Mbit/s link 28.5 / 42.8, 0 % lost; unshaped 34.6 / 91.4, 13.7 % lost, 19 freezes / 5.9 s. The unshaped and 150 Mbit/s 4K arms lost their first keyframe and latched Wi-Fi pacing (finding 4); the 300 Mbit/s arm did not. 4K picture age is dominated by the NUC's VAAPI decoder (12 to 19 ms a frame against a 16.7 ms budget), not by the link.

Also at a fitting bitrate: a Wi-Fi-like profile (150 Mbit/s, 3 ms +-2 ms jitter, 0.5 % random loss) shows 16.9 / 29.9 ms at 20 Mbps and 18.1 / 31.1 ms at 60 Mbps, 0 frames lost, FEC recovering about 2400 frames in 40 s and no unrecoverable frame. A 5 ms queue on a 150 Mbit/s link costs nothing (12.9 / 26.2, 0 % lost); on a 50 Mbit/s link it drops the startup keyframe, the loss latch fires, pacing falls to 30 Mbps and the picture age rises from 16.6 to 21.1 ms (4 unrecoverable frames, no frame lost in the end).

Step changes in one stream (20 s per step, rate changed on the live shaper):

| Stream | Steps (Mbit/s) | Result |
| --- | --- | --- |
| 20 Mbps | 300, 30, 300 | invisible: 0 % lost, 0 freezes, picture age p95 24 to 27 ms in the 5 s after each step |
| 20 Mbps | 80, 10, 80 | 29.3 % lost, one 20.1 s freeze; picture back within about 1 s of the rate returning |
| 40 Mbps | 300, 30, 300 | 29.0 % lost, one 20.1 s freeze, 148 recovery keyframes; back within about 1 s |
| 40 Mbps | 80, 10, 80 | 29.6 % lost, one 20.2 s freeze |

Frames shown per second, steps at second 21.8 and 41.8 (20 Mbps, 80, 10, 80): 60 until the drop, 47 in the second of the drop, 0 for 19 s, 7 in the second the rate returns, 61 in the next.

### Findings, ranked by user impact

1. **A stream above its link never recovers, and the host never lowers the bitrate.** At 1.5x the link (225 Mbps on 150, 450 on 300) 97 to 99 % of frames are lost, and at 120, 75, 45 and 40 Mbps on 80, 50, 30 and 20 Mbit/s no frame arrives for the whole 40 s. The host keeps sending what it was told (`host_sent_mbps` 226.9 on the 225 Mbps arm). In code the loss response only changes pacing, never the encoder: `core/src/keyframe_loss.rs:365` (`bps` returns `base.min(safe)`), applied at `host/src/stream.rs:1871` and `host/src/pyrowave_send/transport.rs:72`. `host_adaptive_bitrate` is the only path that lowers the bitrate: it is opt-in, depends on client loss reports and switches `keyframe_loss_pacing` off (`core/src/config.rs:355-377`, `core/src/adaptive_bitrate.rs:1-6`). Any client that picks a bitrate its link cannot carry (a phone on a weak hotspot, a wrong link estimate) stays on a black or frozen picture until the user restarts at a lower rate. Suggested fix: let the loss latch lower the encoder bitrate too: when it is latched and unrecoverable frames continue for a few seconds, step the bitrate down without waiting for client reports, and raise it back slowly (the controller in `adaptive_bitrate.rs` already has the budget math, and with `host_adaptive_bitrate = true` it fixes these arms; see below).
2. **A link that falls below the stream's bitrate mid-stream freezes the picture for as long as it lasts.** 80 to 10 Mbit/s with a 20 Mbps stream, and 300 to 30 with a 40 Mbps stream, give a 20 s freeze each (0 fps from second 22 to 41) and about 30 % lost overall. The same step to a rate the stream still fits (300 to 30 with 20 Mbps) is invisible. Recovery after the rate returns is about 1 s. Same cause and fix as 1.
3. **A link close to the stream's bitrate is the worst case.** On a 20 Mbit/s link a 20 Mbps stream (plus 20 % FEC) loses 12 to 36 % of frames with 15 to 63 freezes; H.264 is worst (36 %, 63 freezes / 14.9 s, 189 IDR requests). Picture age is 54 to 59 ms median. The stream is not far above the link, so the latch (pacing at about 30 Mbps) cannot help. Same fix as 1; the bitrate also has to leave room for FEC: `WireBudget::video_bps` (`core/src/adaptive_bitrate.rs:43`) accounts for it, but only when adaptation is on.
4. **A lost startup keyframe latches slow pacing for a day, even on a fast link.** A keyframe that arrives incomplete sets the latch (`core/src/keyframe_loss.rs:346`). In this run: 4K at 150 Mbit/s and on 1 GbE (2 of 3 4K arms) and a 5 ms queue on 50 Mbit/s. The latch is remembered for 24 h per client uuid and address, the next four sessions start at the Wi-Fi rate and the fifth probes the fast rate (`host/src/state/loss_memory.rs:15-16`). Measured cost on a 1 GbE client: median picture age of HEVC 21 ms instead of 13, AV1 12.5 instead of 5, H.264 21 instead of 13, p99 33 instead of 26 (first run of the matrix, `C:\src\rl-link-work\m-memory`, loss memory left on; it is the confound the table above removes). Nothing in the client says why latency got worse; `POST /api/clients/reset-network-history` clears it. Suggested fix: remember a latch only when it came from sustained loss (several frames, or loss after the startup keyframe), not from one startup keyframe, and probe the fast rate on the second session when the first showed no further loss.
5. **The host's startup pacing is blind to the client's link.** A wired or unknown route paces at 800 Mbps (`core/src/network_pacing.rs:22-24`) and the host saw `link_bps=2500000000`, its own NIC, in every arm. The first keyframe leaves as a burst the 20 to 150 Mbit/s queue cannot hold, which is what feeds finding 4 (the drop-tail queue was 50 ms of the link rate; a smaller queue hits harder, as the 5 ms arm shows). Suggested fix: pace the first keyframe at a conservative rate and step up once frames arrive complete, or use a client-reported hint as `pyrowaveLinkMbps` already does for PyroWave.
6. **Picture age rises as the link tightens, even without loss.** HEVC median 12.9 ms at 150 Mbit/s, 14.2 at 80, 16.6 at 50, 20.9 at 30 (AV1 5.1, 6.0, 8.0, 11.7). Likely serialization of the 20 Mbps stream at the lower rates; not separated further. Minor, but it adds to finding 3.
7. **Unshaped 1 GbE with a 300 Mbps stream loses 1.5 % (p99 138 ms), and a 500 Mbps stream 7 % (p99 181 ms).** The container bridge path is included and not separately attributable.

What the numbers do not show: no link rate down to 30 Mbit/s hurts the default 20 Mbps stream beyond a few ms of picture age; Wi-Fi-like jitter and 0.5 % loss are absorbed by FEC; a 4K stream at 80 Mbps fits 150 Mbit/s. The risk is a bitrate the link cannot carry, and the host has no way out of it on its own.

### With `host_adaptive_bitrate = true`

The same host with the opt-in option on (`m-abr`, one run each, fixed 45 s): the collapses in findings 1 and 2 disappear, at the price of a low bitrate.

| Arm | Picture age p50 / p99 | Lost | Freezes | Host sent (Mbps, mean) | Encoder bitrate seen |
| --- | --- | --- | --- | --- | --- |
| 225 Mbps on 150 Mbit/s | 25.8 / 39.6 | 0 % | 0 | 35.4 | 179 Mbps at start, 24.7 Mbps after 9 s (one cut a second, 10 cuts), 30 Mbps at the end; the link carries about 100 Mbps |
| 120 Mbps on 80 Mbit/s | 27.0 / 77.7 | 0 % | 0 | 39.5 | 95 Mbps at start, 59 Mbps after the first second, 10 Mbps at the end; the link carries about 54 Mbps |
| 20 Mbps on 20 Mbit/s | 25.5 / 41.1 | 0 % | 0 | 11.0 | 7.7 to 15 Mbps |
| 20 Mbps, 80 to 10 to 80 | 22.3 / 38.7 | 1.5 % | 3 / 1.2 s (longest 0.76 s) | 8.1 | 15 Mbps to 3.4 Mbps in 4 s at 10 Mbit/s (about 6 Mbps fits); after the link returned to 80 it climbed 0.1 Mbps every 2 s to 4.2 Mbps, then cut again to 2.4 Mbps and never got back above 4.2 |

Without it the same arms lose 99 %, 100 %, 15 % and 29 % with 20 s freezes. So the controller already does what finding 1 asks for, and `nightjar-cli` gives it the feedback it needs (`feedback_observed: true`). What it costs: it cuts too far and does not come back. Every arm starts at the requested bitrate and falls, one cut a second (`host adaptive bitrate old_kbps new_kbps` lines in each `host-events.log`), and the cuts after the first are mostly `rising_rtt` in `adaptive_bitrate.reason`, not loss: reports from frames sent before a cut keep arriving, and the controller has no frame number to tell them apart. The 225 Mbps stream ended at 30 Mbps on a link that carries about 100; in the step arm the link returned to 80 Mbit/s at 40.8 s and the controller cut again at 55.6 s (4.2 to 3.4 to 2.9 to 2.4 Mbps in three seconds, all `rising_rtt`) and stayed there. The 20 Mbps stream averaged 11 Mbps on a 20 Mbit/s link and 8 Mbps in the step arm. `keyframe_loss_pacing` is switched off as well. RTT is not a reliable trigger either: it rises with the bottleneck queue the stream itself fills. Not measured: picture age and bitrate on a link that fits the stream (the 25 ms medians here are on links at or below the stream's rate). Suggested next step: make this controller the default for clients that send the feedback, with a faster climb back once frames arrive clean, after measuring a fitting link with it on.

### Not run

Nightjar's automatic bitrate (the CLI has none; static reading of the Android client: 20 % step-downs after two poor samples and at least 3 s since the last change, +5 % after 15 s healthy), PyroWave (the CLI has none), client queue depth and client FEC counters (the CLI does not print them), a 5 ms queue below 50 Mbit/s, 120 fps, and any real Wi-Fi link. At 10 Mbit/s the HEVC arms produced no frame the timing join could use; AV1 and H.264 arms lost every frame.

### Tooling

`rust/release/linkshape.sh` (tc shaper), `linkspeed_client.py` (runs in the client container), `linkspeed.py` (host side: `list`, `shapecheck`, `run ARM... --work DIR [--keep-memory]` from an elevated shell, `report --work DIR`, `compare A_DIR B_DIR [--repeats N]`, `noise DIR...`), `linkcompare.py` (the analysis and verdicts, unit-tested in `test_release.py`). `ARM@2` runs an arm again into its own folder. Raw results are in `C:\src\rl-link-work\m` (clean) and `m-memory`.

### Healthy links and run-to-run noise (3.0.0-beta.1)

Before any link-adaptation change, the harness got nine healthy arms, `hz-r{1000,500,300}-hevc-{20,40,80}` (1080p60 HEVC, noise content, 45 s, loss memory reset), a per-second encoder bitrate and pacing series, and `compare`, which judges a candidate against a baseline (healthy arms: no bitrate change line, one constant encoder bitrate equal to the baseline's, the same `network pacing selected`, |delta p50| at most 0.3 ms, delta p99 at most +1 ms, loss no worse). Picture age here is `e2e_ms` of the A8 timing join. RX 7900 XT host, `butterpollo.exe` 3.0.0-beta.1 package, NUC client (VAAPI), three runs of every arm (the third in reverse arm order), per-run values in `C:\src\rl-link-work\l1\aa`:

| Arm | Picture age p50 (ms), runs 1 / 2 / 3 | p99 (ms) | Lost | Freezes | Host sent (Mbps) | Encoder bitrate | Pacing |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 GbE, 20 Mbps | 13.0 / 13.0 / 14.2 | 26.7 / 26.2 / 28.7 | 0 % | 0 | 18.9 | 14,988 kbps, constant | 800 Mbps |
| 1 GbE, 40 Mbps | 14.1 / 13.9 / 14.3 | 29.6 / 27.9 / 30.9 | 0 % | 0 | 38.8 | 30,988, constant | 800 Mbps |
| 1 GbE, 80 Mbps | 15.4 / 15.3 / 16.0 | 29.5 / 28.4 / 33.9 | 0 % | 0 | 78.8 | 62,988, constant | 800 Mbps |
| 500 Mbit/s, 20 Mbps | 12.8 / 13.9 / 12.9 | 26.0 / 27.7 / 25.0 | 0 % | 0 | 19.0 | 14,988, constant | 800 Mbps |
| 500 Mbit/s, 40 Mbps | 13.6 / 14.8 / 14.8 | 27.2 / 31.6 / 29.7 | 0 % | 0 | 38.9 | 30,988, constant | 800 Mbps |
| 500 Mbit/s, 80 Mbps | 15.2 / 16.1 / 15.7 | 29.7 / 36.2 / 35.9 | 0 % | 0 | 79.2 | 62,988, constant | 800 Mbps |
| 300 Mbit/s, 20 Mbps | 13.1 / 13.4 / 13.1 | 26.8 / 28.9 / 27.1 | 0 % | 0 | 18.9 | 14,988, constant | 800 Mbps |
| 300 Mbit/s, 40 Mbps | 13.7 / 14.8 / 14.0 | 27.7 / 30.5 / 28.1 | 0 % | 0 | 38.8 | 30,988, constant | 800 Mbps |
| 300 Mbit/s, 80 Mbps | 15.7 / 16.6 / 15.7 | 28.3 / 32.5 / 29.8 | 0 % | 0 | 78.8 | 62,988, constant | 800 Mbps |

Noise (`linkspeed.py noise`): the same package against itself differs by 0.05 to 1.2 ms in the p50 of one arm; the pooled run-to-run sigma is 0.51 ms (18 degrees of freedom), the worst arm 0.68 ms, the largest difference between two runs of one arm 1.18 ms. The spread is not a fixed offset between passes (the 500 and 300 Mbit/s arms were 1 ms higher in pass 2, the unshaped 20 Mbps arm in pass 3), so ordering the arms ABBA does not remove it. **A 0.3 ms bound on one arm's p50 cannot be resolved with a few runs: 95 % confidence needs 23 runs per side from the pooled sigma (39 from the worst arm).** `compare` defaults to `--repeats 23` for that reason and refuses to pass an arm with fewer. What does resolve: the bitrate series, the pacing lines, loss and freezes are exactly reproducible (every run above has the same single encoder bitrate and pacing, 0 % lost), and the mean delta over the nine arms, which `compare` prints, has a third of the spread of one arm. Whether the 0.3 ms per-arm bound is kept at 23 runs or replaced by the nine-arm mean is a decision for the acceptance run (B8).

Not run here: PyroWave, real Wi-Fi, the loss-memory scenario (`--keep-memory` is in place for B8).

## October 24: link-rate false cuts and the knee hold (link-fix)

Own measurements on the RX 7900 XT dev box (`linkspeed.py`, release package, noise content, 45 s arms, loss memory reset, NUC client with tc shaping), main against `link-fix`.

- **Wi-Fi arm** (150 Mbit/s, 3 +/- 2 ms delay, 0.5 % loss, HEVC 1080p60 at 14,988 kbps): the old controller counted the data packets a reordering path had not delivered yet when the client reported a rebuilt block (about 14 % of the data) as loss and cut to the 2,415 kbps floor in 15 changes. Now 0 changes, 0 % lost, p50 16.6 ms; a 1.5 s RTT excursion from 20 to 60 ms on the same model is no cut either, and a standing queue with extra loss still is (unit tests).
- **`r50-hevc-hi`** (50 Mbit/s, 50 ms queue, 75 Mbps asked; the limit is under 2 % of frames lost after the start-up and never over 3 %): main loses 6.7 %, with p50 31 to 42 ms. A probe that fails, or a cut with a queue standing in the path, now holds probes under the failed rate for 20 s, so the stream stops climbing into the next queue. Loss by build: 0.74, 1.43, 1.86 and 1.04 % in four runs of the build before the re-review fixes (`cf4366016`, p50 22.6 to 23.2 ms); 1.90 and 1.82 % in two runs of `dc375ec62` (p50 24.9 and 22.5 ms); 1.34, 1.34, 2.69, 2.99 and 2.94 % in five runs of the final build `cb6459a70` (p50 24.1 ms in three, 27.6 and 48.0 ms in the other two, longest freeze 0 to 0.46 s). That build missed: three of five runs were over 2 %, all of it the first four seconds, where the keyframe sent inside the latch's fence counted as arrived and the client's repeated keyframe requests, each one event, did not fail the latch (the first cut came 4.2 s after it). The 48.0 ms run lost 1.34 % and had no loss after its failed probe at 41,606 kbps: it sat at 36,768 kbps, about 47 Mbps on the wire into 50, with a standing queue of 25 ms and no rebuilt blocks, so nothing was left to count and the queue delay alone was the cost of holding that rate. With two events in the latch watch (100 ms or more apart, inside the fence or not) failing the latch, the final build `abfa56315` loses 1.86, 1.69, 1.60 and 1.65 % in four runs (p50 22.4 to 22.5 ms, longest freeze 0.38 to 0.41 s, 1.3 to 1.6 s from the latch to the first cut, then probes back up, one failing in three of the four runs, and 31 to 32 Mbps in the last 20 s); the Wi-Fi arm is still 0 changes, 0 % lost, p50 16.4 ms. Then holding the averaged baseline to the block's parity share (a per-block clip at 0.2 had cut the Wi-Fi model with 30 % FEC and up on a 1.5 s RTT spike) and requiring a watch pair to come within 1.5 s: build `c3c4e09e1` loses 1.95 and 1.65 % in two runs (p50 22.7 and 22.8 ms, first cut 1.3 and 1.6 s after the latch), Wi-Fi 0 changes, 0 % lost, p50 16.5 ms.
- **Cost:** the quiet hold makes a recovery after a link dip slower. In the unit-test simulator (`Sim`, not `linkspeed.py`; `a_link_that_drops_to_an_eighth_recovers_within_twenty_five_seconds`) the stream is back at 90 % of its old rate 13.6 to 21.7 s after the link returns on `link-fix` against 10.6 to 14.1 s on main, up to 8 s later. The test allows 25 s.

## October 24: a lossless standing queue (link-fix, delay signal)

Own measurements on the RX 7900 XT dev box (`linkspeed.py`, release package, noise content, 45 s arms, NUC client with tc shaping). The loss signals cannot see a queue that never drops: a run of the `r50-hevc-hi` arm held 36.8 Mbps into 50 Mbit/s with a 25 ms queue, no loss and p50 48 ms. Every figure below is in `C:\src\rl-link-work`: the result lines in `lr18.log`, the arm's `results.jsonl` in the run's directory (`lr18-*`), the host log copy as `lr18-*.host.log`.

- **Signal:** the ENet RTT against its lowest value of the last 30 s (one-second buckets, the queue's own floor, apart from the all-time minimum that `rtt_elevated` and the reorder baseline use). More than 15 ms above it in every sample for 2 s, while video is busy and no loss evidence fired, is a standing queue. Only a queue the stream can have made counts: one that starts within 8 s of a probe step, or that goes on after a cut. The response is a 12 % step down, a recorded knee (the probe that raised the rate into it is a failed probe, back to the last good rate) and the quiet hold on probes. It then looks at the RTT: falling means wait, flat after a cut means a slower route or another flow, so the queue floor is re-learned and nothing more is cut, rising means another step. At most three cuts a queue (about a third of the rate): a queue three cuts do not drain is another flow's, and probes stay held by `rtt_elevated`. The link notice, the console warning and the stream card say "queue"; the 5 s timings line carries `link_rtt_ms` and `link_rtt_floor_ms`.
- **Unit tests** (`link_rate/tests.rs`; the standing queue, route change and deep-buffer tests fail on the build before): a lossless queue after a probe is cut and the RTT returns near the floor; a route change costs at most one step and a slow RTT growth none; a competing flow that keeps the queue rising gets at most three cuts and no probe on top of it; a deep-buffer link in the simulator (`Sim`, 400 ms drop-tail queue) settles with a queue under 120 ms at p90. The Wi-Fi tests (parity 7 to 16, 700 ms spikes and 1.8 s excursions) pass on both builds and stay no-cut.
- **Cost, Wi-Fi stalls:** a stall of 2.5 to 4 s that starts within 8 s of a probe step is over the 2 s hold and costs one 12 % cut (the probe goes back to the last good rate) and the knee hold, so probes stay under the failed rate for 20 s (unit test `a_wifi_stall_of_a_few_seconds_after_a_probe_costs_one_cut_and_the_knee_hold`). A stall with no probe in the last 8 s costs nothing. A route that stays slower for good holds probes for as long (`rtt_elevated`, as before this signal).
- **Cost, shallow buffers:** on a link with a 50 ms buffer the delay signal can add up to two cuts to the earlier count; the bound in `probes_stop_under_the_rate_that_failed_for_the_quiet_hold` went from 3 to 5 failures (no more than 3 of them on loss).
- **New arm `deep-r50-hevc-hi`** (50 Mbit/s, 400 ms tc queue, 75 Mbps asked), the build before the delay signal (`c3c4e09e1`, main's link code) against the final build, two runs each, alternating:

| Run | Frames lost | p50 | p95 | Settled at | Unrecoverable frames, run end |
|---|---|---|---|---|---|
| before, `lr18-base-deep1` | 2.42 % | 22.7 ms | 389 ms | 29.5 Mbps (loss) | 44 |
| final, `lr18-new-deep1` | 4.02 % | 23.3 ms | 283 ms | 31.9 Mbps (queue) | 107 |
| before, `lr18-base-deep2` | 6.93 % | 22.7 ms | 400 ms | 29.5 Mbps (loss) | 111 |
| final, `lr18-new-deep2` | 3.81 % | 23.6 ms | 336 ms | 31.9 Mbps (queue) | 104 |

  The start-up loss is not caused by the delay signal: all four runs start at the requested rate, overload the 400 ms buffer and lose the first frames before the first cut, and the build before the signal loses 2.4 and 6.9 % (the final build 4.0 and 3.8 %). The two earlier runs of an intermediate build that lost 0 % (`lr15-deep1`, `lr15-deep2`) were not repeated in these four runs; the start-up loss varies from run to run, and we did not find what made those two clean, so treat 0 % as a lucky start-up. After the start-up the before build keeps losing frames (unrecoverable 39 to 44 in run 1, 101 to 111 in run 2) while it probes into the buffer and cuts on loss; the final build cuts the probe on its queue, goes from 41.6 to 36.3 to 31.9 Mbps within 7 s of the probe, and loses no more frames. p50 is the same, within 1 ms; the start-up loss is left as it is. The baseline is two runs a build, so the p95 and loss differences between builds are a trend, not a measurement of a size.
- **Not regressed** (final build): `r50-hevc-hi` 1.17 and 1.08 % lost, p50 23.6 and 22.6 ms (`lr18-new-r50-1`, `lr18-new-r50-2`); Wi-Fi arm 0 changes, 0 % lost, p50 16.6 ms (`lr18-new-wifi`).

## October 24: near-link tuning, three steps tried and reverted (L6)

Three changes to the link-rate controller that apply only while it is `Limited`, each behind an internal config key (`link_rate_pace_limited`, `link_rate_headroom_percent`, `link_rate_tight_encoder`), measured against the unchanged build and kept only if loss or freezes improved and the p50 was no worse:

- **(a) Pace at `max(delivered, 1.25 x needed)`** instead of the delivered rate.
- **(b) Headroom 0.80** instead of the 0.85 the controller already uses (the cut aims at 80 % of the delivered rate).
- **(c) Tight encoder:** AMF peak bitrate equal to the target (ratio 1.0) and a VBV of one frame while limited, restored on `Full`.

None was kept. The keys, the code behind them and the plumbing were removed again (the experiment commit stays in the history).

**Method.** Relative A/B only, on the 780M laptop (RDNA 3 iGPU, over Wi-Fi 6E to the LAN), which is not the release test host and not a gaming PC's absolute numbers. Host on the laptop (isolated profile, `virtual_display_layout = extended`, port 48923), client `nightjar-cli` in the container on the NUC, shaped by `linkshape.sh`. One package per round, the key switched per pass in `sunshine.conf`. Each step was run ABBA with two runs per side. The baseline passes of all rounds are pooled (8 runs per arm). Step (b) got a second ABBA round on the package rebuilt from current main (`abb803c67`, two B runs more, four in all). Arms: `r20-{hevc,av1,h264}-def`, `r30-hevc-def`, `step-80-10-80`, `q5-r50-hevc-def`, and `hz-r1000-hevc-20` as the control. The control had zero link-rate changes, zero adaptive-bitrate changes, one pacing selection and a constant encoder bitrate in all 16 runs, so none of the steps touches a healthy link. Picture age was not available on this client, so p50 and p99 are the end-to-end latency of the timing record.

Cell: lost / freezes / p50 ms / p99 ms / host sent Mbps (means over the runs):

| Arm | baseline (8 runs) | (a) pace (2) | (b) 0.80 (4) | (c) tight (2) |
| --- | --- | --- | --- | --- |
| 20 Mbps HEVC on 20 Mbit/s | 1.94 % / 3.2 / 30.7 / 110 / 11.5 | 0.26 % / 0.5 / 30.0 / 87 / 11.6 | 1.33 % / 2.8 / 28.4 / 77 / 11.6 | 2.39 % / 5.0 / 27.8 / 86 / 10.7 |
| 20 Mbps AV1 on 20 | 1.25 % / 1.2 / 22.0 / 70 / 12.6 | 0.00 % / 0.0 / 21.9 / 26 / 13.8 | 0.00 % / 0.0 / 22.3 / 27 / 14.3 | 0.00 % / 0.0 / 21.9 / 26 / 13.8 |
| 20 Mbps H.264 on 20 | 0.75 % / 1.8 / 26.5 / 86 / 10.1 | 0.67 % / 1.0 / 25.3 / 89 / 9.2 | 1.33 % / 2.8 / 25.5 / 86 / 9.4 | 2.00 % / 4.5 / 26.7 / 86 / 9.7 |
| 20 Mbps HEVC on 30 | 0.18 % / 0.8 / 30.9 / 48 / 18.8 | 0.00 % / 0.0 / 30.3 / 40 / 18.6 | 0.00 % / 0.0 / 31.0 / 41 / 19.0 | 0.34 % / 1.0 / 31.1 / 69 / 17.3 |
| step 80 to 10 to 80 | 1.95 % / 3.9 / 28.6 / 105 / 9.5 | 4.60 % / 7.5 / 31.6 / 225 / 8.5 | 1.72 % / 4.0 / 29.1 / 62 / 11.3 | 1.74 % / 4.0 / 28.1 / 61 / 9.9 |
| HEVC on 50, 5 ms queue | 0 % / 0 / 31.0 / 40 / 18.9 | 0 % / 0 / 30.6 / 41 / 18.6 | 0 % / 0 / 31.3 / 43 / 18.9 | 0 % / 0 / 31.5 / 41 / 19.1 |
| control: 1 GbE, 20 Mbps | 0 % / 0 / 31.2 / 41 / 18.9 | 0 % / 0 / 31.0 / 40 / 18.5 | 0.20 % / 0.8 / 31.5 / 54 / 18.9 | 0 % / 0 / 31.2 / 41 / 19.1 |

Summed over the four limited arms (`r20` x3 and the step): loss 5.89 baseline, 5.53 (a), 4.39 (b), 6.12 (c) percentage points; freezes 10.1, 9.0, 9.5, 13.5; freeze time 2739, 3247, 2442, 3252 ms.

**Decisions.**

- **(a) pace above the delivered rate: reverted.** It helped the three r20 arms in two runs each (HEVC 0.26 % against 1.94 %), but the step arm got worse in both runs (4.60 % against 1.95 %, 7.5 freezes against 3.9, p99 225 ms against 105 ms, p50 +3 ms) and the freeze time summed over the limited arms is higher. Sending faster than the link delivered refills the queue the cut has just emptied.
- **(b) headroom 0.80: reverted.** The first round looked good (summed loss 3.8 against 8.3 for the baseline passes next to it), the confirmation on current main did not (5.0 against 3.9), and H.264 was worse in both (1.33 % against 0.75 %, 2.8 freezes against 1.8). The eight baseline passes sum to 2.1 to 13.3 points of loss over the four limited arms; the four 0.80 passes lie from 3.2 to 5.7, the baseline median is 4.4. The control arm lost 0.78 % in one 0.80 run with no link-rate change, which is what the Wi-Fi hop alone does. A lower spread in bad passes is possible, but twelve passes cannot show it, and each cut costs 6 % of the bitrate for certain.
- **(c) tight encoder: reverted.** Loss and freezes got worse on HEVC, H.264 and the 20-on-30 arm (HEVC 2.39 % against 1.94 %, H.264 2.00 % against 0.75 %, summed freezes 13.5 against 10.1) with a lower mean bitrate on HEVC (10.7 against 11.5 Mbps). A one-frame VBV starves the encoder on a complex frame.

**Caveats.** Two runs per side on a Wi-Fi laptop cannot resolve effects below the run-to-run spread: a single baseline run of `r20-av1-def` lost 0 %, 3.1 % or 7.0 % depending on the pass. Only the sign of large effects is trustworthy, which is enough for (a) and (c), not to call (b) better or worse. The step arm of the first (a) round was invalid (the harness issued the step a minute late because a new ssh connection from the laptop stalled while the stream ran); the harness now sends step commands over one open shell (`fix(release): link speed step arms ...`) and the step arm was rerun for all four (a) passes. A 45 s arm is short for a probe cycle.

## October 24: beam-raced PyroWave on the desktop client (B10 step 4)

Nightjar desktop `b10-step4`, loopback to an isolated Rubylight host (RX 7900 XT), 1080p60 PyroWave
4:2:0, 399 Mbps, pace 900, release builds, `beam_race` on, `NIGHTJAR_STREAM_LOCATION=local` (1360 byte
packets), 60 s, 3589 frames. The frame time on the wire is W = 6.48 ms at p50.

| Measure | Value |
|---|---|
| Frames with `row_ordered` | 100% (3589 of 3589) |
| Slices decoded per frame | 4 on every frame |
| `first_slice_ready - receive` | p50 1.65 ms, p95 2.14 ms (0.25 W) |
| `last_slice_ready - receive` | p50 7.20 ms, p95 7.92 ms |
| Slice decodes against the whole-frame decode | exact on 24 frames (72 slice decodes), all three planes |

The host log shows `pyrowave_records=true pyrowave_row_order=true`. The first quarter of the picture is
on the GPU about 1.65 ms after the frame's first packet, while the frame takes 6.5 ms to arrive. What
this does not show: a presented-picture gain (the present path still shows whole frames), Wi-Fi, or
loss. Loopback counts as a remote link, so without `NIGHTJAR_STREAM_LOCATION=local` the host drops row
order for the 1024 byte packets (a row-ordered frame is about 20% larger there, about 3% at 1360
bytes) and says so with a warning. The presented-picture gain was measured later and missed its default-on gate; see [the gate entry](#october-24-beam-raced-pyrowave-gate-b10-step-4-opt-in-missed-its-default-on-gate) below. The client keeps it as an experimental opt-in.

## October 23: PyroWave desktop decode stage split (brief 1)

Nightjar desktop, PyroWave 4:2:0 at 1080p60, loopback to an isolated Rubylight host (RX 7900 XT, one 5120x1440 display, WGC, a scrolling texture), release builds of Nightjar `pyrowave-decode-1`. Legs of 60 s, the first 5 s dropped, A B B A per bitrate, early decode off. The full tables, the harness (`scripts/pyrowave-loopback.ps1`) and the stage timers are in Nightjar's `docs/pyrowave-decode.md`.

### Where the decode time goes (before any change)

Milliseconds, p50 / p95, one leg per bitrate setting. The host's `sent_mbps` stayed at 83 to 90 Mbps in every leg of both settings, so with this content the two settings did not send different traffic.

| Stage | 133 setting | 300 setting |
| --- | --- | --- |
| entropy decode | 2.45 / 3.42 | 3.02 / 6.93 |
| clear coefficient planes | 0.92 / 1.27 | 0.98 / 1.64 |
| half pack + RGBA16F frame | 5.03 / 8.66 | 5.97 / 13.77 |
| upload to the texture (presenter) | 3.53 / 4.73 | 3.95 / 5.44 |
| 11 wavelet passes (CPU submit time only) | 0.13 / 0.21 | 0.15 / 0.26 |

The same code run offline over 120 real frames (146988 bytes each) gives 7.48 ms median and 15.05 ms p95 for the whole decoder; in the stream entropy and pack cost more at p50 (about 1.2 ms and 3 ms), with the encoding host on the same PC.

### Two presenter changes

- Upload to one texture instead of two: kept. The upload stage fell from 3.2 to 3.6 ms to 1.65 to 1.85 ms p50, `decode_to_present_ms` p50 from 54.0 to 52.4 ms at 133 and 53.8 to 52.3 ms at 300 (the repeat at 300: 53.9 to 52.3 ms). It passed the keep rule at 133 Mbps. At 300 Mbps it failed twice, once on an outlier A leg and once on `receive_to_present_ms` p95 (+2.94 ms) that comes from the decoder queue and not from the presenter. Kept as a judgement call, in its own commit. The ignored test `single_upload_draws_the_same_pixels_as_uploading_both_textures` compares every pixel.
- All eleven passes in one command list: reverted. The submit cost fell from 0.10 to 0.04 ms, `decode_to_present_ms` p50 did not move (52.40 against 52.42 ms at 133).

### The present clock held every frame for 50 ms at the start of a stream

`decode_to_present_ms` sat at 53 to 54 ms in every leg. The cause is not the swap chain: with a display that reports tearing support the pacing is `HostTimes` (the default), vsync is off, and `PresentClock` shows each frame at its host timestamp plus a delay. That delay was seeded from the first frames, which are late while the decoder and GPU warm up (frame 1 was 92 ms late), and it falls at only 25 us a frame, so every frame was held for the 50 ms maximum until it had come down (the median fell from 53.5 ms at 25 s to 35 ms at 40 s in one leg and stayed at 54 ms for 60 s in another). The first fix showed the first 16 pictures as soon as they were ready and kept them out of the lateness window. ABBA at 133 Mbps with that fix (client window inside the capture): `decode_to_present_ms` p50 47.0 to 11.5 ms and `receive_to_present_ms` p50 53.8 to 18.6 ms (p99 61.6 to 29.1 ms); at 300 Mbps `decode_to_present_ms` p50 52.3 to 16.5 ms and `receive_to_present_ms` p50 59.7 to 25.5 ms.

That first rule (show 16 pictures at once) is replaced. The clock now (a) closes the gap between the held delay and the wanted one by dt/500 ms per frame, never slower than 25 us a frame, (b) caps the delay at the window median plus 50 ms, and (c) shows the first 250 ms of host time at ready time plus 0.5 ms without touching the window, so the warm-up does not depend on the frame rate. Regression tests cover a mid-stream stall, a stall every 64 frames, and a 300 ms start burst at 144 fps. The earlier fps failure (shown fps 50.6 against 55.8 at 133, 53.1 against 57.8 at 300) came from the harness: with one display the client window was inside the host's capture, so the client's own drawing fed back into the stream. With the window excluded from the capture (`SetWindowDisplayAffinity`, `WDA_EXCLUDEFROMCAPTURE`) the repeat ABBA, 60 s legs, first 5 s dropped, 8 legs, gives KEEP at both bitrates, where A is the first-16 rule and B the new clock:

| Setting | Build | decode_to_present p50 / p95 (ms) | receive_to_present p50 / p95 / p99 (ms) | Shown fps |
| --- | --- | --- | --- | --- |
| 133 | A (2 legs pooled) | 54.70 / 58.31 | 64.44 / 79.73 / 94.21 | 55.71 |
| 133 | B (2 legs pooled) | 11.50 / 18.13 | 21.33 / 28.03 / 29.66 | 60.20 |
| 300 | A (2 legs pooled) | 54.11 / 56.10 | 67.81 / 140.00 / 309.03 | 58.90 |
| 300 | B (2 legs pooled) | 13.13 / 31.01 | 27.90 / 47.12 / 55.12 | 60.18 |

Frames that reached the decoder: 3013 to 3587 in A legs, 3581 to 3587 in B legs; host `sent_mbps` 140 (133) and 316 to 317 (300) in both. Every B leg is within 11.5 to 13.7 ms at p50, while the same A build gave 56, 47 (133) and 55, 31 ms (300) in the four A legs, because its delay depends on how long ago the last bad stretch was; one A leg (300 Mbps, first) had `receive_to_present` p95 284 ms with `queue_wait_ms` p95 217 ms, which we did not trace. A first ABBA with the window excluded streamed about 20 fps because the animation script did not compile on PowerShell 7.4; it was discarded and the script fixed. HEVC 1080p60 at the default bitrate, one leg each way: `decode_to_present` p50 / p95 8.54 / 17.78 ms (A) against 8.11 / 13.94 ms (B), `receive_to_present` p50 / p95 / p99 8.83 / 18.08 / 25.09 against 8.40 / 14.25 / 15.75 ms, shown fps 60.11 against 60.21, 3587 against 3592 frames. Four legs per bitrate on one PC is a small sample. Not measured: H.264, a stream started from the GUI, a display without tearing support (vsync pacing does not use the clock), early decode with slice delivery on.

## October 23: PyroWave 144 fps pacing and async encode (B4)

Three open questions from the PyroWave audit, answered with the A8 timing record. Result: the 2.5.6 pacing change stays reverted, async encode is not worth building, and the handle growth is a fixed cost of the SDK and the AMD Vulkan driver that a new test now bounds.

### 144 fps pacing: normal against high process priority

**What was tried.** The change reverted in 2.5.9 ([#42](https://github.com/RamazanKara/Rubylight/issues/42)): 2.5.6 stopped raising the host to `HIGH_PRIORITY_CLASS` while streaming and relied on the early-arming timer wait (`Timer::until_paced`) alone. It was re-implemented as `stream_normal_priority` (off by default), measured, and removed again because it did not measure better. No code from it is in the tree.

**Method.** RX 7900 XT, release build of main at 8d03c8cc8 plus the one setting, isolated profile, loopback, `rust/release/e2e.py --record` with the PyroWave receiver, 1920x1080 at 144 fps, 400 Mbps, 60 s per run, ABBAAB order (A is the shipped high priority, B is normal; the process priority of each host was read back at 30 s). The capture display runs at 240 Hz, so presents arrive at about 8.1 ms median with a 9.4 ms p99 even in the best case; that is the floor for the jitter column. Two batches: idle, and under load (12 spinning threads plus `gpu_load 60 1000 60 200`, started after the PyroWave capability probe finished, because a loaded probe timed out). Three runs per arm; means below, per-run values in `C:\src\b4-work\idle-*` and `load-*` on the dev box. A first batch ran by accident with both arms below normal and was discarded.

| Idle, 3 runs per arm (mean) | A high (shipped) | B normal |
| --- | --- | --- |
| Delivered fps | 144.09 | 144.09 |
| Present interval p50 / p99 (ms) | 8.12 / 9.36 | 8.07 / 10.49 |
| Present interval max (ms) | 17.5 | 17.5 |
| Arrival gaps over the harness limit (count) | 4.0 | 6.7 |
| end_to_end median / p95 / p99 (ms) | 6.56 / 10.85 / 11.79 | 6.65 / 10.99 / 12.14 |
| host_total p99 (ms) | 9.89 | 10.15 |
| Picture age mean / p99 (ms) | 12.94 / 17.50 | 13.07 / 18.60 |
| Host frames replaced | 0 | 0 |

| Under load, 3 runs per arm (mean) | A high (shipped) | B normal |
| --- | --- | --- |
| Delivered fps | 143.33 | 142.90 |
| Present interval p50 / p99 (ms) | 6.78 / 14.52 | 6.84 / 15.66 |
| Arrival gaps over the harness limit (count) | 15.3 | 19.7 |
| end_to_end median / p95 / p99 (ms) | 8.75 / 14.48 / 18.01 | 8.58 / 15.12 / 19.54 |
| Picture age mean / p99 (ms) | 20.25 / 65.28 | 21.85 / 86.82 |
| Host frames replaced | 0 | 0 |

Normal priority was equal or worse on present jitter p99, arrival gaps, end_to_end p95/p99 and picture age in both batches; the only point in its favour is a 0.17 ms lower median under load, inside the run-to-run spread (8.59-8.97 ms for A). It had no jitter gain even with an idle PC, so there is no case for keeping it: kept only if no worse on every metric and better on jitter, and it is neither. This agrees with the field report in #42 and the 2.2.9 notes. Runs with rc=1 failed only the harness's "excessive gaps" check (an artefact of 144 fps on a 240 Hz display, present in both arms and in the idle A arm) or, under load, the receiver falling behind; their metrics were used. No run exited with code 3 (no encoder hang).

### Async encode: not worth it

`examples/performance.rs`, release build, PyroWave 400 Mbps, synthetic frames, RX 7900 XT, `encode_call` is the whole GPU-side call: conversion, submit, the CPU wait on the timeline fence, packetizing.

| Run | Frames/s | encode_call p50 / p99 / max (ms) |
| --- | --- | --- |
| 1080p, unpaced (ceiling, repeat frame, 11.7 KB per frame) | 2,860 / 2,877 | 0.34 / 0.49 / 0.77 |
| 1080p, paced 144 fps (261 KB per frame) | 144 | 0.81 / 0.94-0.99 / 1.1-1.2 |
| 1440p, unpaced | 2,632 | 0.37 / 0.51 / 1.07 |
| 1440p, paced 144 fps (264 KB per frame) | 144 | 0.92 / 1.11 / 1.15 |

In the 144 fps e2e runs above the encode stage median is 1.06 ms (0.97 ms under load) and the whole host stage p99 is 9.9 ms. At 144 fps the frame interval is 6.94 ms, so the encoder thread waits on the GPU for at most 12-13% of it (0.81 and 0.92 ms), and the GPU is busy for no more than that. Overlapping that wait with the next frame's capture or conversion needs a next frame to exist, and at 144 fps the next present is still 6 ms away when the encode finishes; the capture-paced loop is idle, not blocked, for the other 87%. Letting two frames be in flight would only help when frames are already queued, which means the pipeline is behind, and it adds one frame (6.9 ms) of latency or buffering to that case. Packetizing and sending already run on their own threads after the encode. So there is no latency to win at 144 fps, and the ceiling is 2,600-2,900 fps at 1080p and 1440p, 18-20 times the target. Not prototyped.

### Handles left by each encoder or decoder life

Roughly 4 handles per life, measured with a handle count across lives in a new ignored GPU test (`pyrowave_lives_do_not_leak_handles`, 50 lives per case, growth counted after the first 10):

| Life | Handles per life |
| --- | --- |
| D3D11 capture device and upload only | 0 |
| Encoder built | 4.0 (161 over 40 lives) |
| Encoder, one frame, graphics conversion | 4.0 (160) |
| Encoder, one frame, D3D12 compute conversion | 4.0 (160) |
| Decoder built | 4.0 (160) |

The four are 3 events and 1 thread handle, the same count for the encoder and the decoder. The process thread count does not grow (3 to 4 threads over 10 lives), so they are handles left behind while the pinned SDK builds its Vulkan objects on the AMD driver (where exactly was not narrowed down), not threads we hold. Our D3D devices, the shared-handle imports (Granite closes those) and the compute conversion add nothing, and the SDK source has no thread or event creation of its own in the encoder or decoder. It cannot be fixed on our side without patching the driver, and it costs 4 handles per stream start, which a host would need millions of streams to notice. The test fails if a life leaves more than 4, so a leak of ours is caught.

## October 23: Rust engine against the C engine on Android (B8)

Nightjar's new streaming engine (`checkbox_rust_engine`, Rust) against the current engine (C, moonlight-common-c), on the same phone, host, codec and picture. By measurement the Rust engine is no worse than the C engine on picture age, decode time, loss recovery and a 30-minute soak. The default is not flipped here; that is the owner's call. Two things need a follow-up first: the Rust engine writes no clock offset into its latency CSV, and its overlay shows a different RTT (see "Caveats").

**Fixture.** Retroid Pocket 6 (Snapdragon, adb `e24bb88e`), Nightjar `dev-retroid` debug build with the B8 gap fixes (Nightjar main `6456da5d` and later), Wi-Fi at -41 to -43 dBm, link 1080 to 1200 Mbps. Host: isolated profile on 192.168.4.69:48123 (RX 7900 XT, AMF, WGC, `virtual_display_layout = extended`, audio off, `video_slice_delivery = off`) running the package from the B2 phone work. Picture: `motion_probe` with 1024 noise on the captured display. Compact overlay on. The engine was confirmed on every run from the logcat line `Streaming engine: C (moonlight-common-c)` or `Rust (nightjar-core)` and the overlay. Runs took the GPU-LOCK one group at a time and the main host was idle before each.

**Method.** The client latency CSV joined to the host timing record (`BUTTERPOLLO_TIMING_RECORD`) with `timing_report.py`, first 2 s of every run dropped. The C engine writes `clock_offset_us` and `clock_rtt_us`, so its join is `clock_sync` (bound about 1 ms). The Rust engine writes neither, which would drop its join to a `min_owd` lower bound that cannot be compared with the C one. So the phone-to-PC clock (phone clock minus PC QPC, which drifts at a steady 34.9 us/s, checked over 14 runs) was fitted on the C runs and written into copies of the Rust CSVs. Leave-one-out error of that fit: median -115 to +64 us, p5 to p95 about +-0.5 ms. Differences under about 0.5 ms in `end_to_end` therefore need the clock-free rows to confirm them: `assemble`, `client_total` and `decode` are measured on the phone only.

### HEVC 1080p60, 40 Mbps, 4 runs of 75 s per engine (ABBA)

| ms, median / p95 / p99 | C | Rust | Rust minus C (median) |
| --- | --- | --- | ---: |
| end_to_end (joined, picture age) | 7.04 / 10.42 / 15.98 | 6.92 / 10.23 / 15.61 | -0.12 |
| network_last (host first packet to client last packet) | 1.72 / 2.91 / 4.33 | 1.50 / 2.69 / 4.10 | -0.22 |
| decode (`input_to_output`) | 0.92 / 1.26 / 1.47 | 0.92 / 1.27 / 1.46 | 0.00 |
| host_total | 3.49 / 6.87 / 12.49 | 3.56 / 6.87 / 12.41 | +0.07 |
| assemble (client, clock-free) | 0.33 / 1.08 / 2.00 | 0.12 / 0.82 / 1.76 | -0.21 |
| client_total (client, clock-free) | 1.72 / 2.64 / 3.62 | 1.53 / 2.39 / 3.37 | -0.19 |

- Per-run `end_to_end` medians: C 7.24, 7.06, 7.00, 6.89; Rust 6.88, 7.01, 6.85, 6.91 (the unfitted lower-bound join put Rust 0.46 ms behind, which was the join, not the engine).
- Frames: 17,523 (C) and 17,520 (Rust) joined of 18,003 and 18,000 sent. Dropped by the client: 2 (C, 0.011%) and 4 (Rust, 0.022%). CSV rows lost: 0 in both. Host `idr_requests` 1 and `reference_invalidations` 0 in every run, so no recovery keyframes.
- Host fps median 59.35 (C) and 59.32 (Rust), the same capture rate in both.

### Loss recovery

`BUTTERPOLLO_TEST_SEND_LOSS=<percent>:42` on the host (random drop of video datagrams, same seed), one 75 s run per engine and loss rate, HEVC 1080p60 at 40 Mbps.

| Run | Engine | Packets FEC had to rebuild | Frames FEC rebuilt | Host `idr_requests` / `reference_invalidations` | Host frames the client never showed | `end_to_end` median / p95 / p99 (ms) | Longest gap between received frames (ms) |
| --- | --- | ---: | ---: | --- | ---: | --- | ---: |
| 1% | C | 1972 | 1561 | 1 / 0 | 0 | 7.28 / 10.94 / 16.89 | 23.7 |
| 1% | Rust | 1950 | 1573 | 1 / 0 | 0 | 6.98 / 10.66 / 16.58 | 29.2 |
| 5% | C | 10551 | 3813 | 4 / 3 | 3 | 7.56 / 10.65 / 15.97 | 49.8 |
| 5% | Rust | 10363 | 3830 | 1 / 0 | 0 | 7.30 / 10.62 / 16.16 | 25.4 |

- At 1% both engines recovered every frame from FEC. No recovery keyframe, no skipped frame.
- At 5% the C run lost three frames (client CSV frame numbers 1507 to 1509, 1809 to 1811, 3892 to 3894) and asked for a reference invalidation each time, which the host answered without a keyframe. Each loss cost one skipped frame, no stall: the next frame decoded and displayed normally. The Rust run lost none. One run per engine, so 3 against 0 lost frames out of 4,490 is not a significant difference; it is a "no worse".
- `fec_unrecoverable_frames` was 0 in all four runs and `send_loss_recoveries` 0. No visible corruption on the mid and end screenshots of any of the four runs (noise and checker picture stayed clean), no decode errors in logcat.
- Recovery time: the longest gap between received frames never exceeded 50 ms (one C outlier at 49.8 ms), so every loss healed within about three frame intervals.

### PyroWave, 1080p60, 400 Mbps, 75 s per engine

Nightjar has no PyroWave-specific default bitrate (the stored bitrate follows resolution and frame rate, 20 Mbps at 1080p60, below PyroWave's floor), so the value is the one `docs/video-and-latency.md` gives as the start for 1080p/60. The host streamed `codec=3` at 398.7 Mbps (configured 400,000 kbps) in both runs. Host `pyrowave = true`, adaptive FEC not negotiated by the client in either run (host warning, same in both).

| ms, median / p95 / p99 | C | Rust | Rust minus C (median) |
| --- | --- | --- | ---: |
| end_to_end | 36.19 / 43.76 / 62.55 | 34.99 / 41.79 / 57.44 | -1.20 |
| to_decode (end_to_end minus display) | 16.05 / 25.07 / 44.02 | 15.29 / 20.20 / 36.75 | -0.76 |
| decode | 3.05 / 3.45 / 3.77 | 3.04 / 3.37 / 3.64 | -0.01 |
| display | 19.72 / 25.10 / 26.62 | 19.23 / 25.47 / 26.66 | -0.49 |
| assemble | 8.75 / 12.46 / 17.13 | 7.99 / 11.00 / 14.13 | -0.76 |

Frames dropped by the client: 0 in both (4 rows each end with `stream_ended`, from leaving the stream). Host frames the client never showed: 16 (C) and 10 (Rust), counted at the end of the run, includes the frames in flight when the stream ended. One run per engine.

### Soak: Rust engine, HEVC 1080p60, 40 Mbps, 30 minutes

- Frames: 107,513 sent, 14,915 joined. The client latency CSV holds the last 14,915 frames only (about the last 4 minutes), so the picture-age trend covers the last four minutes, not all 30. Host `stream timings` cover all 30 minutes (357 five-second lines).
- Fps: host median 59.33 per 5 s window (C 75 s runs 59.35), minimum window 57.34. Mean per fifth of the run: 59.37, 59.29, 59.27, 59.35, 59.34, 59.36, 59.29, so steady. Client drops 0, CSV rows lost 0, `present_drops` 0.
- Picture age over the last 4 minutes, per minute (median, p95, p99 ms): 7.57 / 18.41 / 20.22, then 7.39 / 10.55 / 16.28, 7.34 / 10.75 / 16.79, 7.38 / 10.65 / 16.10, and 7.34 / 11.64 / 19.94 for the 515 frames of the partial last minute. The median is flat and equals the 75 s runs. The first minute's p95 of 18 ms is one burst; I did not trace it, but the host logged one FEC-unrecoverable frame and three reference invalidations over the 30 minutes (host `idr_requests` 2, `fec_unrecoverable_frames` 1, no injected loss), so it is plausibly that.
- Memory (`dumpsys meminfo`, total PSS): 153.3 MB at the start, then 148.3, 157.1, 154.5, 155.0, 154.6 MB at 5-minute steps; no growth. A last sample 22 s after the stream ended read 156.1 MB. Java heap 37.7 MB at the start and 38.2 MB at the last in-stream sample; native heap 21.5 and 21.2 MB.
- Errors: no error-level lines in the 30-minute logcat, no ANR or crash, no warning or error in the host log. The stream ended only when the run was stopped.
- Thermal: the thermal service reported status 0 throughout. Battery temperature 33.0 C at the start, 34.0 C from the 25-minute sample on; battery level 85 percent to 79 percent.

### Caveats

- The Rust engine leaves `clock_offset_us` and `clock_rtt_us` empty in the CSV. The `end_to_end` figures above used a clock fitted on the C runs (leave-one-out error up to +-0.5 ms), so the 0.12 ms median gap is inside that error; the clock-free `assemble` and `client_total` rows (0.2 ms lower on Rust) and the equal decode time are the firmer evidence. Filling the clock columns in the Rust engine would make a future comparison direct.
- The compact overlay RTT reads 2 ms on the C engine and 7 to 8 ms on the Rust engine on every screenshot taken (pilot runs, the 5% runs, the soak end), while the host measured 2.1 ms for both. The Rust engine's overlay RTT is not the same quantity as the C engine's, or is not right; not investigated here.
- One run per engine for loss and PyroWave, one phone, one Wi-Fi spot, one picture. 75 s runs were not repeated at different times of day.
- Not measured: HDR, 144 Hz, 4K, AV1, a phone on a poor link, controller input latency.

### Verdict

Rust is no worse than C on picture age (median, p95 and p99 equal or lower in HEVC and PyroWave), decode time (equal), loss recovery (no more skipped frames, no corruption at 1% and 5%) and the 30-minute soak (steady fps, flat memory and latency, no errors). By the plan's rule the default can flip. Do the two caveats first (Rust engine writing the clock columns, and the overlay RTT), so the flip does not leave an unexplained number on screen.

## October 23: 4:2:0 against packed 4:4:4 on text and UI, and AFMF capture (B3, B7), measured on the 780M

Two questions from the 3.0 plan, both measured on the laptop's Radeon 780M (RDNA3 iGPU, Windows 11, AMD Software 26.10.1, a 2880 px wide panel). Not the RX 7900 XT: every number here is the 780M's and says nothing about a discrete card. Tools: `chroma_fixtures.py` and `chroma_quality.py` in `rust/release`, and the `amf_quality`, `afmf_scene`, `afmf_capture` and `radeon_set` examples, release build of branch b3-b7-laptop.

### B3: should packed 4:4:4 be the Reference preset's default

**Verdict: no. The default stays as it is.** Packed 4:4:4 is clearly better on still text above about 5 Mbit/s of actual bitrate (8-bit HEVC; scrolling and the first frame gain less or lose), but it costs 1.2 to 2.1 ms more encode time and 0.7 to 1.2 ms more hardware decode, spends about four times the bits at a 20 Mbit/s target, and is worse than 4:2:0 at 2 Mbit/s. That is not "a small latency increase" and it is not better everywhere, so `stream_presets.json` is unchanged.

**Fixture.** Three deterministic 90-frame scenes drawn by `chroma_fixtures.py` with sub-pixel text (a code editor, a browser article, a spreadsheet): 30 frames still, 30 scrolling 6 px a frame, 30 still. 1080p and 1440p, 8-bit HEVC through AMF (`amf_quality`, 60 fps, Rec.709 limited, packing 0 = 4:2:0, 1 = packed 4:4:4), targets 2, 5, 10, 20, 50 and 100 Mbit/s, one encode per cell (72 runs; each row below is the mean of the three scenes). `chroma_quality.py score` decodes with FFmpeg, upsamples a 4:2:0 picture or unpacks a packed one, and compares 13 sampled frames per run with the source in full-resolution YCbCr: PSNR and SSIM per plane, and PSNR in RGB. "Settled" is the final still frame (frame 89 of 90); "scroll" is the mean of the sampled scroll frames and "first frame" is frame 0. Encode time is `amf_quality`'s frame latency, which starts at the colour conversion and packing, so the packing cost is in it.

1080p:

| Target (Mbit/s) | Mode | Actual (Mbit/s) | Settled Y / U / V (dB) | Settled RGB (dB) | Scroll RGB (dB) | First frame RGB (dB) | Encode mean / p99 (ms) |
| ---: | --- | ---: | --- | ---: | ---: | ---: | --- |
| 2 | 4:2:0 | 1.6 | 47.1 / 37.3 / 37.5 | 33.2 | 31.9 | 24.3 | 4.90 / 9.6 |
| 2 | packed | 2.0 | 35.0 / 39.7 / 39.8 | 32.1 | 28.4 | 23.8 | 6.11 / 10.7 |
| 5 | 4:2:0 | 1.7 | 48.5 / 37.4 / 37.6 | 33.3 | 33.1 | 27.5 | 4.90 / 9.4 |
| 5 | packed | 4.9 | 43.6 / 44.6 / 44.6 | 38.8 | 32.3 | 25.7 | 6.15 / 11.0 |
| 10 | 4:2:0 | 2.4 | 54.4 / 37.6 / 37.9 | 33.7 | 33.5 | 30.6 | 4.90 / 9.6 |
| 10 | packed | 7.7 | 47.3 / 46.1 / 46.2 | 41.1 | 34.7 | 29.0 | 6.15 / 11.1 |
| 20 | 4:2:0 | 3.4 | 57.3 / 37.7 / 38.0 | 33.8 | 33.6 | 31.9 | 4.88 / 9.4 |
| 20 | packed | 13.4 | 53.7 / 51.2 / 51.2 | 46.4 | 37.3 | 33.8 | 6.17 / 10.8 |
| 50 | 4:2:0 | 4.4 | 58.0 / 37.7 / 38.0 | 33.8 | 33.7 | 32.4 | 4.91 / 9.5 |
| 50 | packed | 25.9 | 55.5 / 55.2 / 55.2 | 49.4 | 42.1 | 37.8 | 6.12 / 10.8 |
| 100 | 4:2:0 | 4.7 | 58.1 / 37.7 / 38.0 | 33.8 | 33.7 | 32.6 | 4.87 / 9.4 |
| 100 | packed | 42.5 | 56.7 / 56.6 / 56.6 | 50.4 | 47.5 | 39.2 | 6.13 / 10.5 |

1440p:

| Target (Mbit/s) | Mode | Actual (Mbit/s) | Settled Y / U / V (dB) | Settled RGB (dB) | Scroll RGB (dB) | First frame RGB (dB) | Encode mean / p99 (ms) |
| ---: | --- | ---: | --- | ---: | ---: | ---: | --- |
| 2 | 4:2:0 | 1.8 | 46.1 / 38.2 / 38.5 | 34.0 | 32.2 | 25.0 | 7.37 / 12.1 |
| 2 | packed | 2.0 | 33.8 / 39.6 / 39.8 | 31.3 | 28.4 | 24.8 | 9.49 / 13.7 |
| 5 | 4:2:0 | 2.2 | 49.3 / 38.5 / 38.8 | 34.4 | 34.2 | 27.2 | 7.41 / 12.1 |
| 5 | packed | 5.0 | 40.5 / 43.0 / 43.1 | 36.6 | 32.0 | 25.6 | 9.41 / 13.7 |
| 10 | 4:2:0 | 2.1 | 49.7 / 38.5 / 38.8 | 34.4 | 34.3 | 30.1 | 7.36 / 11.9 |
| 10 | packed | 9.0 | 46.8 / 46.5 / 46.6 | 41.2 | 34.9 | 28.2 | 9.39 / 13.8 |
| 20 | 4:2:0 | 3.2 | 56.3 / 38.8 / 39.1 | 34.8 | 34.8 | 32.8 | 7.39 / 12.3 |
| 20 | packed | 12.8 | 48.6 / 47.2 / 47.3 | 42.2 | 37.0 | 32.2 | 9.54 / 13.7 |
| 50 | 4:2:0 | 5.7 | 59.0 / 38.9 / 39.1 | 34.9 | 34.9 | 33.6 | 7.31 / 11.9 |
| 50 | packed | 27.3 | 56.3 / 54.6 / 54.6 | 49.3 | 41.4 | 39.4 | 9.42 / 14.0 |
| 100 | 4:2:0 | 5.7 | 59.3 / 38.9 / 39.1 | 34.9 | 34.9 | 33.8 | 7.28 / 12.0 |
| 100 | packed | 46.7 | 57.4 / 57.1 / 57.1 | 51.1 | 46.3 | 40.6 | 9.39 / 13.6 |

Client decode, hardware HEVC through D3D11VA on the same 780M (FFmpeg `-hwaccel d3d11va`; the bitstream repeated 10 and 40 times so the difference removes start-up, 900 and 3600 frames; the 20 Mbit/s runs; three repeats agreed to 0.02 s). This is a proxy for Nightjar's decoder, not Nightjar itself:

| Picture | 4:2:0 | Packed (twice the height) | Difference |
| --- | ---: | ---: | ---: |
| 1920x1080 editor | 0.96 ms | 1.65 ms | +0.69 ms |
| 2560x1440 editor | 1.51 ms | 2.71 ms | +1.21 ms |
| 2560x1440 spreadsheet | 1.50 ms | 2.71 ms | +1.21 ms |

**What it shows.**

- **4:2:0 has a ceiling on text.** Its settled RGB PSNR stays at 33.7 to 33.8 dB (1080p) from a 10 Mbit/s target up and 34.8 to 34.9 dB (1440p) from 20, however high the target: the chroma planes stay near 37.7 and 38.9 dB while luma climbs to 58. The encoder also stops spending: at a 20 Mbit/s target the 4:2:0 stream used 3.4 Mbit/s (1080p) and 3.2 Mbit/s (1440p) on these mostly still scenes.
- **Packed 4:4:4 passes it from about 5 Mbit/s of actual bitrate.** At a 5 Mbit/s target the settled RGB PSNR is 38.8 dB against 33.3 dB (1080p); at 20 Mbit/s 46.4 against 33.8 dB; at 50 Mbit/s 49.4 against 33.8 dB. Chroma (U) SSIM goes from 0.948 to 0.995 at 20 Mbit/s.
- **At 2 Mbit/s it is worse.** Packed 4:4:4 is 1.1 dB lower in settled RGB at 1080p and 2.7 dB lower at 1440p, and its luma is 12 dB lower, because the same bits cover twice the picture.
- **Scrolling gains less and later.** Scroll RGB is 0.8 dB lower at a 5 Mbit/s target, 1.2 dB higher at 10, 3.7 dB higher at 20 (37.3 against 33.6 dB, 1080p) and 8.4 dB higher at 50.
- **The first frame is not better below 20 Mbit/s.** First-picture RGB is 0.2 to 1.9 dB lower with packed 4:4:4 at 2 to 10 Mbit/s (and 0.6 dB lower at 1440p, 20 Mbit/s); it is 1.9 dB higher at 1080p, 20 Mbit/s and 5.4 dB higher at 50. A keyframe is bit-limited in both modes.
- **Cost.** Encode +1.2 ms at 1080p (4.9 to 6.1 ms) and +2.1 ms at 1440p (7.4 to 9.4 ms), p99 +1.4 ms at 1080p and +1.7 ms at 1440p (means over all six targets); client hardware decode +0.7 and +1.2 ms. Together roughly 1.9 ms at 1080p and 3.3 ms at 1440p of added picture time from these two stages, before the Nightjar reconstruction shader (not measured). At a 20 Mbit/s target the stream spends 12.8 to 13.4 Mbit/s against 3.2 to 3.4.

**Why the default stays.**

- The latency increase is not small next to the 7.06 ms end-to-end median the 780M measured over the LAN (6-minute 1080p60 HEVC, [plan](../docs/rubylight-3.0-plan.md), B9 row), and the rule is no added latency without a clear win.
- The gain exists only when the link and the bitrate target are generous; at 2 Mbit/s it is a loss. A preset cannot know the link.
- The advertisement is host-wide: `experimental_packed_444` is probed at startup and again when it is saved, once no stream runs (`rust/host/src/state.rs`), and the client has to request the packing, so a per-stream preset value cannot switch it on by itself. Making it a preset value would need a startup-level change, which is a separate piece of work.
- Not measured: the Nightjar reconstruction shader on a phone, picture age end to end with a real client, AV1, and real desktop capture. The scenes are synthetic (sub-pixel text on flat backgrounds), which is close to the best case for the chroma ceiling.

**10-bit and the layered packing.** The same scenes with `--ten-bit` (packing 0 = 4:2:0, 1 = packed version 1, 2 = layered version 2), targets 5, 20 and 50 Mbit/s, 54 runs, scored against the 10-bit reference. Means over the three scenes, settled RGB PSNR, scroll RGB PSNR and encode time:

| Size | Target (Mbit/s) | 4:2:0 actual / settled / scroll / encode | Packed v1 actual / settled / scroll / encode | Layered v2 actual / settled / scroll / encode |
| --- | ---: | --- | --- | --- |
| 1080p | 5 | 1.6 / 33.3 / 33.1 dB / 4.78 ms | 4.9 / 39.0 / 32.4 dB / 6.23 ms | 3.5 / 38.2 / 36.0 dB / 6.91 ms |
| 1080p | 20 | 2.6 / 33.8 / 33.7 dB / 4.78 ms | 12.9 / 46.6 / 37.4 dB / 6.23 ms | 7.5 / 43.5 / 43.1 dB / 6.87 ms |
| 1080p | 50 | 3.0 / 33.8 / 33.7 dB / 4.81 ms | 23.6 / 49.6 / 42.3 dB / 6.23 ms | 11.7 / 47.3 / 46.8 dB / 6.91 ms |
| 1440p | 5 | 2.1 / 34.4 / 34.2 dB / 7.35 ms | 4.9 / 37.2 / 32.1 dB / 9.92 ms | 4.3 / 38.3 / 35.2 dB / 11.25 ms |
| 1440p | 20 | 2.7 / 34.8 / 34.8 dB / 7.34 ms | 13.4 / 42.2 / 37.0 dB / 9.84 ms | 5.5 / 39.9 / 39.7 dB / 11.30 ms |
| 1440p | 50 | 3.6 / 34.9 / 34.9 dB / 7.38 ms | 26.3 / 49.6 / 41.5 dB / 9.91 ms | 14.1 / 46.7 / 46.4 dB / 11.19 ms |

Version 2 reaches 38 to 47 dB settled with 3.5 to 14 Mbit/s and keeps the scroll picture 2.7 to 5.7 dB better than version 1 at a 20 Mbit/s target, but its encode time is 2.1 to 4.0 ms above 4:2:0 (against 1.4 to 2.6 ms for version 1); the client's reconstruction cost was not measured. It does not change the verdict: the added encode time is larger, not smaller. Whether it is the better low-bitrate packing is a question for a link-limited run, which this fixed-target test does not answer.

Raw runs and scores: `C:\src\rl-b3b7-work\out\sdr` (`runs.jsonl`, `scores.jsonl`, the bitstreams); the decode bench output is `C:\src\rl-b3b7-work\out\decode-bench.txt` (its printed ms-per-frame column used unreliable frame counts; the table above uses the known 900 and 3600 and the rtime values).

### B7: does AFMF reach the stream

**Verdict: unresolved, and no Radeon control is added.** AFMF can be switched on and restored through the existing ADLX code on the 780M, but it did not engage for the test app, so no captured picture was ever a generated one, and the latency of generated frames could not be measured.

**Method.** `afmf_scene` is a borderless, top-most, full-display D3D11 flip-model window with `Present(1, 0)` and a frame-latency waitable object, capped to 30, 60 or 240 fps. It draws a 64 px bar that jumps 48 px per frame and the frame number as 16 bits, so a rendered picture reads back its own number and a generated one shows a bar between two positions or a blend. `afmf_capture` captures the same display with WGC or Desktop Duplication through the host's capture module and classifies every captured picture as rendered, generated or unreadable. `radeon_set` writes `afmf.enabled` through the journaled controller and restores it. The engagement test is the app's own pacing. This is an assumption, not something measured here: that AFMF holds the app to about half the display rate, so an app that could run faster than the 120 Hz refresh would drop to about 60. The scene's bar jump and full-screen scroll may also be fast enough that AFMF's fast-motion handling falls back, which would look the same.

| AFMF | Capture | App cap (fps) | App rendered fps | Captured fps | Rendered / generated / unreadable |
| --- | --- | ---: | ---: | ---: | --- |
| off | WGC | 240 (held at 120 by vsync) | 119.86 | 120.06 | 1321 / 0 / 0 |
| off | Desktop Duplication | 240 | 119.86 | 120.06 | 1321 / 0 / 0 |
| off | WGC | 30 | 30.05 | 30.00 | 330 / 0 / 0 |
| on | WGC | 240 | 119.79 | 120.07 | 1321 / 0 / 0 |
| on | Desktop Duplication | 240 | 119.71 | 120.04 | 1321 / 0 / 0 |
| on | WGC | 60 | 60.05 | 60.08 | 661 / 0 / 0 |
| on | WGC | 30 | 30.05 | 30.09 | 331 / 0 / 0 |
| on | Desktop Duplication | 30 | 30.05 | 30.09 | 331 / 0 / 0 |
| off again | WGC | 240 | 119.87 | 120.06 | 1321 / 0 / 0 |
| on, no capture running | none | 240 | 119.72 | n/a | n/a |
| on, no capture running | none | 60 | 60.07 | n/a | n/a |

`afmf.enabled` read back true while on, the restore returned cleanly, and nothing was pending afterwards. With it on the app was not slowed (119.7 against 119.9 fps), so there was no sign that the driver took over the pacing (under the assumption above), and not one captured picture had a bar between positions. Exclusive fullscreen was tried on this panel and DXGI refused it (`SetFullscreenState` returned `0x887A0001` every time; observed during the work but the output was not saved), so the borderless path is the only one tested.

**What this does and does not say.** On the 780M with this driver, a borderless D3D11 flip-model window showed no sign of frame generation, so WGC and Desktop Duplication captured only the app's own frames in every cell. It does not show that AFMF output is invisible to capture: AFMF may work for a game the driver recognises or in a mode this laptop could not offer, or with slower motion than this scene's bar jump and full-screen scroll (AFMF reduces generation on fast motion), and then the answer could differ. The IddCx virtual display path was not run, because without a generated frame there was nothing to compare. No latency figure for AFMF is claimed.

Raw reports: `C:\src\rl-b3b7-work\out\afmf` (`*.capture.json`, `*.scene.json`, `matrix.log`).

## October 23: stream presets (B3)

`stream_preset = lowest_latency | balanced | reference` ([docs/configuration.md](../docs/configuration.md#stream-presets)) on loopback with the timing record, to check that each preset reaches the encoder and what it costs or saves. Verdict: **pass**. All 13 runs applied the properties the preset names (read back from the host's "AMF encoder settings" line), and the startup keyframe is 29% (HEVC) and 38 to 40% (H.264) smaller with `lowest_latency`, with no measurable change in median end-to-end latency.

**Fixture.** RX 7900 XT, the release build of main at 10aa9f0be (`butterpollo.exe` from the gate, copied into a package directory of the previous package), isolated profile, WGC capture of the physical 5120x1440 monitor, the independent moonlight-common-c receiver decoding with D3D11VA, `e2e.py --record --noise-height 1024 --seconds 60`, the preset through `--config stream_preset=<name>`. HEVC at 3840x2160x60 requested at 80 Mbps (the encoder target after the host's FEC share is 62.988 Mbps) and H.264 at 1920x1080x60 requested at 20 Mbps (target 14.988 Mbps). Order for each codec: lowest_latency, balanced, reference, reference, balanced, lowest_latency, so each preset has two runs in a mirrored order; one more HEVC `lowest_latency` run with 1% random send loss (`BUTTERPOLLO_TEST_SEND_LOSS=1:7`, the host's own send-loss rule). Host and receiver share the QPC clock (bound 0.0 ms). The previous slice-delivery section's moving strip with per-pixel noise is the picture, so frame sizes sit near the frame budget.

| Codec | Preset | Runs | Frames | end_to_end median / p95 / p99 (ms) | Largest keyframe (bytes) | Largest other frame (bytes) | `MaxAUSize` applied (frame budgets) | Quality / pre-analysis | Pass |
| --- | --- | ---: | ---: | --- | ---: | ---: | --- | --- | --- |
| HEVC 4K60 80 Mbps | lowest_latency | 2 | 7,163 | 12.40 / 14.99 / 20.18 | 294,321 | 188,481 | 1,049,800 bits (1.0): yes | speed / off: yes | yes |
| | balanced | 2 | 7,200 | 12.48 / 14.66 / 22.07 | 413,265 | 192,513 | 2,099,600 bits (2.0): yes | speed / off: yes | yes |
| | reference | 2 | 7,194 | 12.33 / 14.28 / 21.32 | 413,265 | 188,481 | 0 (no limit): yes | speed / off: yes | yes |
| | lowest_latency, 1% loss | 1 | 3,597 | 12.36 / 15.81 / 23.15 | 295,329 | 191,505 | 1,049,800 bits (1.0): yes | speed / off: yes | yes |
| H.264 1080p60 20 Mbps | lowest_latency | 2 | 7,195 | 4.88 / 7.27 / 11.16 | 50,386 | 43,330 | 249,800 bits (1.0): yes | speed / off: yes | yes |
| | balanced | 2 | 7,197 | 4.95 / 7.57 / 12.31 | 81,634 | 44,338 | 499,600 bits (2.0): yes | speed / off: yes | yes |
| | reference | 2 | 7,189 | 4.81 / 6.99 / 8.47 | 81,634 | 40,306 | 0 (no limit): yes | speed / off: yes | yes |

Latency figures are the mean over the preset's runs of each run's median / p95 / p99; keyframe columns are the largest over its runs. "Applied" means the driver's read-back in the host log at stream start: `HevcMaxAUSize` or `MaxAUSize` equal to the frame budget (`TargetBitrate` divided by the frame rate) times 1, 2 or 0, `HevcQualityPreset=10` (HEVC speed) or `QualityPreset=1` (H.264 speed), and `HevcEnablePreAnalysis=0` or `EnablePreAnalysis=0`. Balanced matches the host's earlier automatic two-frame cap and `reference` reads back 0, the driver default. "Pass" is the harness verdict (exit 0, `passed`, no decode failure) plus those properties. Every run had one keyframe, the startup one (`frame_type` 1 in the receiver's `frames.csv`), 0 decode failures, no exit code 3 and no encoder-hang line; the log warnings were the harness's own (the probe's launch authorisation, the audio fixture ending early in several runs, and in the loss run the send-loss notice and one UDP send warning with one dropped datagram).

**What it shows.**

- **The startup keyframe is the one that moves.** One-frame cap: 294,321 and 294,321 bytes (1% loss run 295,329) against 413,265 and 413,265 for balanced and 412,257 and 413,265 for reference, so 29% smaller in HEVC. H.264: 50,386 and 49,378 against 81,634 and 81,634 (balanced) and 80,626 and 81,634 (reference), 38 to 40% smaller. Reference (no cap) and balanced (two-frame cap) gave the same keyframe, so the two-frame cap did not bind on this picture at startup; only the one-frame cap did.
- **Steady frames are unchanged.** The largest non-key frame is 151 to 193 KB (HEVC) and 40 to 44 KB (H.264) in every preset, and the median frame is 130 KB and 31 KB in all of them.
- **No latency difference we can measure.** The medians sit within 0.15 ms (HEVC 12.33 to 12.48 ms, H.264 4.81 to 4.95 ms), which is below the run-to-run spread; p99 swings more than that inside one preset (HEVC `lowest_latency` 24.7 ms in one run and 15.6 ms in its mirror; H.264 `balanced` 16.8 and 7.9 ms). The claim for `lowest_latency` is smaller keyframes with the same steady stream, not lower latency.
- **1% loss** with `lowest_latency` (one run, HEVC): median 12.36 ms, p99 23.15 ms, no more than the 0% runs of the same preset; it was not repeated.

**Not measured.** Recovery keyframes under load (the October 7 run covers a one-frame cap for HEVC, 66% smaller largest frame at 4K40), AV1 and PyroWave, other bitrates and resolutions, a game as the picture, and a stream over a real link. Raw runs: `C:\src\butterpollo-release\b3-presets` (`presets.jsonl` with one summary per run, and `runs\e2e-<name>` for the logs, records and `frames.csv`).

## October 23: slice delivery (B2 step 2f)

`video_slice_delivery = 4` against `off` ([docs/b2-slice-delivery.md](../docs/b2-slice-delivery.md), sections 6 and 8), measured on loopback at 3840x2160x60, 80 Mbps, 60 s a run, with the timing record. Verdict: loopback and quality pass; **the paced Wi-Fi emulation misses the 1 ms gate** (0.7 ms typical), and **real Wi-Fi (the laptop, Nightjar desktop, below) clears it at 1.2 to 1.4 ms**. Kill criterion 1 needs "less than 1 ms on emulated and real Wi-Fi"; real Wi-Fi is not under 1 ms, so criterion 1 is not met and the feature is not killed. It passes the 1 ms gate on real Wi-Fi and loopback, and misses it only on the emulation.

**Fixture.** RX 7900 XT, release package of main at 36c3c8ac3, isolated host profile on port 48523, `virtual_display_layout` unchanged from `e2e.py` (the physical 5120x1440 monitor, letterboxed into the 4K stream), WGC capture, the independent moonlight-common-c receiver decoding with D3D11VA, `e2e.py --record --noise-height 1024 --mode 3840x2160x60 --bitrate 80000 --seconds 60`. The moving strip carries per-pixel noise so the encoder spends the bitrate (77 to 80 Mbps sent, ~165 KB a frame); that makes the AMF encode 6.2 ms, against 2.6 ms for the 1080p baselines above, and it is the time between the first and the last slice that bounds the gain. `video_slice_delivery` and `pacing_max_bitrate_kbps` went in through `e2e.py --config`, the loss through `BUTTERPOLLO_TEST_SEND_LOSS=<percent>:7` in the environment (the host ignores nothing it inherits), so the harness needed no change. Each condition ran off, 4, 4, off (ABBA) with the same loss seed; every run is 3,590 to 3,601 frames, so each arm has at least 7,000. Host and receiver share the QPC clock (`shared_clock`, bound 0.0 ms). The Nightjar gradle daemons were running on this PC throughout (CPU load only); the ABBA order is there to cancel it.

| Run | Arm | Runs | Frames | end_to_end median / p95 / p99 (ms) | Host frames the receiver never showed | FEC-unrecoverable frames | `slice_delivery_overflows` | Decode errors |
| --- | --- | ---: | ---: | --- | ---: | ---: | ---: | ---: |
| HEVC, 0% loss | off | 2 | 7189 | 12.27 / 14.76 / 22.07 | 6 (0.08%) | 0 | 0 | 0 |
| | 4 | 2 | 7193 | 10.82 / 12.01 / 16.64 | 5 (0.07%) | 0 | 0 | 0 |
| HEVC, 1% loss | off | 2 | 7195 | 12.43 / 14.65 / 21.26 | 5 (0.07%) | 0 | 0 | 0 |
| | 4 | 2 | 7195 | 10.98 / 12.98 / 20.50 | 8 (0.11%) | 0 | 0 | 0 |
| H.264, 0% loss | off | 2 | 7172 | 11.42 / 14.16 / 21.05 | 4 (0.06%) | 0 | 0 | 0 |
| | 4 | 2 | 7184 | 9.94 / 12.62 / 19.89 | 7 (0.10%) | 0 | 0 | 0 |
| H.264, 1% loss | off | 3 | 10781 | 11.24 / 13.54 / 19.35 | 11 (0.10%) | 0 | 0 | 0 |
| | 4 | 2 | 7182 | 9.76 / 11.30 / 16.08 | 7 (0.10%) | 0 | 0 | 0 |
| HEVC, 1% loss dropped in the air | off | 2 | 7200 | 12.22 / 14.07 / 19.00 | 7 (0.10%) | 0 | 0 | 0 |
| | 4 | 2 | 7195 | 10.84 / 12.16 / 20.00 | 4 (0.06%) | 0 | 0 | 0 |
| HEVC paced at 160 Mbps, 0% loss | off | 8 | 28711 | 17.69 / 21.01 / 27.35 | 27 (0.09%) | 0 | 0 | 0 |
| | 4 | 8 | 28719 | 16.72 / 20.00 / 26.52 | 26 (0.09%) | 0 | 0 | 0 |
| HEVC paced, 1% loss | off | 2 | 7195 | 17.57 / 20.12 / 23.87 | 5 (0.07%) | 0 | 0 | 0 |
| | 4 | 2 | 7189 | 16.85 / 19.68 / 24.24 | 7 (0.10%) | 0 | 0 | 0 |
| HEVC paced, 1% loss in the air | off | 2 | 7189 | 17.57 / 20.03 / 24.35 | 7 (0.10%) | 0 | 0 | 0 |
| | 4 | 2 | 7183 | 17.05 / 19.82 / 26.56 | 6 (0.08%) | 0 | 0 | 0 |
| HEVC, 5% loss (extra, not a gate) | off | 2 | 7196 | 12.13 / 13.35 / 18.13 | 4 (0.06%) | 0 | 0 | 0 |
| | 4 | 2 | 7192 | 10.82 / 12.13 / 16.77 | 9 (0.13%) | 8 | 0 | 0 |

Each figure is the mean over the arm's runs of that run's median / p95 / p99. Loss without "in the air" is refused at the socket (the host knows and applies its own send-loss rule); "in the air" counts the datagram sent and drops it, as a lossy link does. The "never showed" column is dominated by the last three to four frames of every stream, which the host sends after the receiver has gone (0% loss shows the same 0.06 to 0.1%); the loss runs add nothing beyond it. One H.264 1% run (on, second) failed the harness's audio-tone check (the tone fixture ran dry once, a known CPU-contention flake); its video stages are normal, it is left out of the table, and a third pair (on, off) replaced it. Its off arm has three runs because of the replacement pair.

**Gain by run (median `end_to_end`, off minus 4):**

| Run | Gain, median of the arm's run medians | Gate |
| --- | ---: | --- |
| HEVC loopback, 0% / 1% / 1% in the air / 5% | 1.45 / 1.45 / 1.38 / 1.31 ms | |
| H.264 loopback, 0% / 1% | 1.48 / 1.50 ms | |
| HEVC paced, 0% (8 runs an arm) | 0.68 ms (mean 0.97) | miss: gate 1 ms |
| HEVC paced, 1% / 1% in the air | 0.72 / 0.52 ms | miss: gate 1 ms |

In the eight paced 0% runs the "4" arm sat at 16.90 to 17.18 ms in seven and 14.69 ms in one; the off arm sat at 17.56 to 17.92 in all eight. The mean gain of 0.97 ms comes from that single run.

**Stages (HEVC, mean of per-run median / p95 / p99, ms).** The new `overlap` stage is the time the first block was on the wire before the encoder had finished: 2.07 ms on loopback and 2.15 ms paced, the encoder's slice lead, which bounds the gain (section 7 estimated up to 3.6 ms paced from a 4.1 ms lead; the lead is 2.1 ms with this content).

| Stage | Loopback off | Loopback 4 | Paced off | Paced 4 |
| --- | --- | --- | --- | --- |
| encode | 6.32 / 6.65 / 6.91 | 6.20 / 6.37 / 6.50 | 6.21 / 6.58 / 6.84 | 6.12 / 6.35 / 6.61 |
| packetize_pace (to first packet) | 0.37 / 0.52 / 0.57 | 1.56 / 1.69 / 1.75 | 0.25 / 0.32 / 0.36 | 1.48 / 1.63 / 1.94 |
| overlap | none | 2.07 / 2.18 / 2.27 | none | 2.15 / 2.29 / 2.49 |
| send (first to last packet, after the encoder) | 1.01 / 1.30 / 1.58 | 0.05 / 0.07 / 0.09 | 7.53 / 8.68 / 9.09 | 6.00 / 7.06 / 7.59 |
| host_total | 8.15 / 10.46 / 17.71 | 6.59 / 7.69 / 12.36 | 14.63 / 17.45 / 23.33 | 12.70 / 15.50 / 21.91 |
| assemble (first packet to assembled) | 0.98 / 1.36 / 1.64 | 2.21 / 2.32 / 2.41 | 6.38 / 7.03 / 8.10 | 8.16 / 9.20 / 9.96 |
| decode | 4.29 / 4.54 / 4.94 | 4.24 / 4.46 / 4.72 | 4.30 / 4.79 / 6.07 | 4.31 / 4.72 / 6.16 |
| end_to_end | 12.27 / 14.76 / 22.07 | 10.82 / 12.01 / 16.64 | 17.69 / 21.01 / 27.35 | 16.72 / 20.00 / 26.52 |

The H.264 stages match the HEVC ones (overlap 2.0 ms).

**Quality (SSIM, slices 1 against 4, fixed bitrate).** `amf_quality.exe` (the encoder `amf_sweep.py` drives) on 90 BGRA frames of two game clips (45 each, with a hard cut between them), `--config` with `video_slice_delivery = off` or `4` (which asks AMF for four slices and slice output, as a stream does), then `ffmpeg` `ssim` and `psnr` of the decoded stream against the source converted to BT.601 limited-range 4:2:0. The encoder is deterministic: the second pass of each arm was byte-identical. The four slices were confirmed in the output (each P-frame came back as four units, 357 units for 90 frames).

| Stream | Slices | Actual kbps | SSIM (all) | PSNR avg dB |
| --- | ---: | ---: | ---: | ---: |
| HEVC 4K60, 80 Mbps | 1 | 77,057 | 0.994715 | 49.555 |
| | 4 | 77,085 | 0.994708 | 49.544 |
| H.264 4K60, 80 Mbps | 1 | 72,156 | 0.993528 | 48.126 |
| | 4 | 72,945 | 0.993398 | 47.977 |
| HEVC 1080p60, 20 Mbps | 1 | 21,554 | 0.989360 | 43.931 |
| | 4 | 21,436 | 0.989271 | 43.887 |
| H.264 1080p60, 20 Mbps | 1 | 21,174 | 0.988790 | 43.444 |
| | 4 | 21,117 | 0.988549 | 43.344 |

The largest SSIM loss is 0.00024 (H.264 1080p60), a tenth of the 0.002 kill line; the 4K H.264 arm also spent 1.1% more bits. Two clips and 90 frames: it is a bound for these pictures, not for every game.

**Checks.** Across the 54 runs (about 180,000 frames, plus a 4-run pilot): no run exited with code 3 or showed an encoder hang; every frame the receiver got was decoded with no decode failure; no "different picture type on a later slice" warning (the later-slice IDR check), no byte-cap overflow, no "never finished" frame, no dropped encoded frame, so `slice_delivery_overflows` was 0 (the counter is read from these warnings; the status JSON is not polled by the harness). `overlap` was positive in every "4" run, so the slices did leave early and the host did not fall back to whole frames.

**Real Wi-Fi (laptop, Nightjar desktop).** The same host build streamed over the air to the 780M laptop (Wi-Fi 6E, 5 GHz channel 116, 802.11ax, 2402 Mbps both ways, RSSI -37 at the start, middle and end of every run), running Nightjar desktop (d3d11va decode) with its latency CSV joined to the host timing record by the clock-sync join (bound 0.48 to 0.51 ms, the same offset on both arms, so it cancels in the difference). The host was wired (Ethernet) at 192.168.4.69, release package of main, isolated profile paired once and kept between arms (the host was stopped, `video_slice_delivery` flipped and the host restarted for every run). Client settings: 3840x2160, 60 fps, HEVC, 80,000 kbps, ABR off; the laptop decoded 4K60 HEVC cleanly (decode 0.33 to 0.35 ms median), so there was no 1440p fallback. The picture was the loopback `motion_probe` with the noise strip (1024 rows) on the captured primary display: 60.1 fps received in every run. Order off, 4, 4, off, off, 4, 4, off (ABBA twice), 75 s a run, 4,527 to 4,610 client frames a run, 18,257 (off) and 18,205 (4) in total. No loss was injected; the link itself dropped nothing the receiver could see.

| Arm | Runs | Frames | end_to_end median / p95 / p99 (ms) | to_decode median / p95 / p99 (ms) | overlap median (ms) | Host frames the client never showed | Decoder errors |
| --- | ---: | ---: | --- | --- | ---: | ---: | ---: |
| off | 4 | 18,257 | 18.68 / 22.15 / 25.33 | 13.20 / 17.43 / 19.12 | none | 0 | 0 |
| 4 | 4 | 18,205 | 17.27 / 20.03 / 23.95 | 11.79 / 16.01 / 17.09 | 2.23 | 0 | 0 |

Per run, median `end_to_end` in ms: off 18.31, 18.30, 18.49, 20.15; 4 16.99, 17.22, 17.23, 17.54. **Gain (off minus 4): 1.41 ms pooled medians, 1.18 ms median of the run medians; p95 2.12 ms, p99 1.38 ms. `to_decode` (`end_to_end` less the `display` stage, which ends at present, and so comparable to the loopback receiver that stops at decode): 1.40 ms pooled, 1.32 ms per-run.** The "4" arm is better in each pair; the largest off run (20.15 ms, p99 26.8) is a slow-display run, and without it the gain is 1.1 to 1.3 ms. The stages move as on loopback: `host_total` 11.83 to 10.61 ms, `send` 0.87 to 0.04, `packetize_pace` 0.12 to 1.39, `assemble` 0.67 to 2.04; network (first and last packet, "4" against off) is 0.94 against 0.97 and 0.72 against 0.84, so the gain is the encoder's slice lead, not a faster link. `overlap` was positive in every frame of every "4" run (`first_unit_us` present on all but the first of each stream), so the slices left early. No run exited with code 3 or showed an encoder hang; no "different picture type on a later slice" warning, byte-cap overflow, "never finished" frame or dropped encoded frame (`slice_delivery_overflows` 0); every client row was `rendered`.

A first ABBA set was invalid and is not used: the harness wrote `video_slice_delivery = on`, not `4`, so both arms ran off. It stands as an off-against-off control: 0.07 ms pooled, -0.05 ms between per-run medians, and one run at 49.7 ms median with a 50 ms display stage (the first of the set, cause not investigated), which is the run-to-run noise to compare 1.2 to 1.4 ms with. The gain is measured on a quiet link (RSSI -37, 2.4 Gbps), one laptop, one room.

**Phone (Retroid Pocket 6, Nightjar Android).** The same host build streamed over the air to a Retroid Pocket 6 (Android 13, Nightjar debug build, hardware HEVC decode, frame pacing set to latency) on the 5 GHz network (channel 116, 5580 MHz; RSSI -40 to -42 at the start, middle and end of every run; link rate 960 to 1441 Mbps transmit and 720 to 1080 Mbps receive as the phone reported it), joined to the host timing record by the clock-sync join (bound 0.89 to 1.07 ms, larger than the laptop's 0.5 ms and the same on both arms). The host was wired at 192.168.4.69 with the isolated profile; the host was restarted with `video_slice_delivery` set for every run. Client: 1920x1080, 60 fps, HEVC forced, 40,000 kbps (the host encoded 31.0 Mbps of video; on the wire 38.2 Mbps with off and 40.5 Mbps with 4 once FEC and the early blocks are counted). Picture: `motion_probe` on the captured display, a 1024-pixel noise strip animated at 120 Hz. Order off, 4, 4, off, 4, off, off, 4; 4 valid runs of 75 s per arm, the first 120 frames of each run dropped. Two 30 s pilots set the bitrate and are not counted, and a first pilot that streamed against a leftover host from an earlier pairing (the harness's stop used a stale pidfile) was discarded; the harness now finds and stops every host of this package by path.

| Arm | Runs | Frames | end_to_end median / p95 / p99 (ms) | decode median (ms) | network_last median (ms) | assemble median (ms) | overlap median (ms) | Host frames the client never showed | Frames dropped by the client | Decoder errors |
| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| off | 4 | 17,509 | 7.03 / 10.35 / 15.52 | 0.90 | 1.75 | 0.33 | none | 0 | 1 | 0 |
| 4 | 4 | 17,496 | 6.86 / 10.09 / 15.54 | 0.93 | 1.41 | 0.85 | 0.93 | 8 of 17,984 | 3 | 0 |

Per run, median `end_to_end` in ms: off 7.18, 6.84, 6.87, 7.18; 4 6.76, 6.97, 6.79, 6.90. **Gain (off minus 4): 0.17 ms pooled medians, 0.18 ms median of the run medians, p95 0.25 ms, p99 -0.02 ms. This misses the 1 ms gate and is inside the noise: the four off runs alone span 0.34 ms.** The phone gets the same network gain per frame as the laptop (`network_last` 1.75 to 1.41 ms), but the 1080p encode takes only about 3 ms, so the encoder's slice lead (`overlap`) is 0.93 ms against 2.23 ms at 4K, and the client's `assemble` stage grows from 0.33 to 0.85 ms (more, smaller pieces to put together), which cancels most of what the network gives back. `host_total` is 3.47 ms off and 3.77 ms with 4. No overflow warning, no byte-cap, no "never finished" frame, no encoder hang or exit code 3, no decoder error in the logcat, 60 fps on screen, and a screenshot of the noise strip with 4 shows no visible corruption. Not explained: 8 host frames in the 4 arm have no client row (none with off; they may be frames in flight at the end of a run, not checked), and one 4 run had 3 IDR requests where the others had 1. One phone, one room, 1080p only; this says nothing about a 4K-capable phone or a slower encoder where the lead is longer.

**Findings.**

- **Loopback gain is 1.3 to 1.5 ms, as estimated** (section 7: ~1.5 ms), for both codecs, at 0%, 1% and 5% loss.
- **Paced gain is 0.5 to 0.7 ms typical**, not 3.6. The wire time shrinks by 1.5 ms (`send` 7.5 to 6.0), but the receiver now starts assembling about 1.8 ms sooner (`assemble` 6.4 to 8.2), so the picture is only complete about when it was. The slice lead is 2.1 ms, not the 4.1 ms the step-1 probe measured on a quieter picture.
- **1% loss does not stress either arm.** At 20% FEC and 4 blocks of about 36 packets, 1% random loss never exceeds a block's parity: no frame was unrecoverable in any run, and p99 is within run-to-run noise in every pair (paced 1%: 23.87 off against 24.24 on, then 24.35 against 26.56 in the air; the off arm's own 0% p99 runs from 26.1 to 31.7). The 5% extra run shows what smaller blocks cost: 8 FEC-unrecoverable frames (6 and 2) in the "4" arm against none in off. It recovered through the host's send-loss path and a keyframe request, and no frame was shown corrupted.
- **Moonlight's host latency reading rises.** The frame header's host processing latency (`host_mean_ms` in `result.json`) is 0.55 ms for whole frames and 2.7 ms (p99 3.0 to 3.9) for streamed ones. A stock client's overlay would show about 2.2 ms more host latency while the picture is 1.4 ms younger.

**Not measured.**

- Phones (Nightjar on Android and stock Moonlight) are still pending: the user runs them. The laptop gives the real-Wi-Fi reading above; a phone decodes and presents differently.
- The NUC over Ethernet was not run: the brief makes it conditional on runs 1 to 3 all passing, and run 2 missed its gate.
- Real Wi-Fi with loss, a congested channel or a weak signal: the laptop run is a quiet 2.4 Gbps link at RSSI -37.
- AV1 and PyroWave (never streamed), HDR, a game as the picture instead of the noise strip, 1080p streams, the 1% gate on a link with bursty loss, and any bitrate other than 80 Mbps.
- Raw runs are under `C:\src\butterpollo-release\b2-2f` (`loop`, `paced`, `air`, `paced2`, `stress`, `rerun`, `ssim`; one `*.jsonl` of per-run summaries each; the real-Wi-Fi runs are under `C:\src\butterpollo-release\b2-wifi`, with the invalid first set in `invalid-run1`).

## October 23: per-frame timing record baselines

These are the first per-frame host-to-client stage times from the timing record (`BUTTERPOLLO_TIMING_RECORD`, [docs/timing-record.md](../docs/timing-record.md)). All runs were on the RX 7900 XT with the loopback receiver and isolated test hosts, using a release package of main with the timing record. Each run was 60 s at 1080p60 and the default bitrate. Host and receiver share a clock (`shared_clock` join, bound 0.0 ms), and every host row matched a client row. Each figure is the median over passing runs of each run's median / p95 / p99, in ms.

| Stage | H.264 (2 runs) | HEVC (2) | AV1 (3) | PyroWave (3) |
| --- | --- | --- | --- | --- |
| Encode | 2.59 / 3.35 / 3.80 | 2.56 / 3.38 / 3.85 | 2.28 / 2.85 / 3.28 | 0.93 / 1.39 / 1.55 |
| Host total (present to last sent) | 3.67 / 10.62 / 11.64 | 3.63 / 10.54 / 11.65 | 4.24 / 10.85 / 11.67 | 11.36 / 18.02 / 18.90 |
| Send (first to last packet) | 0.001 / 0.001 / 0.002 | 0.001 / 0.001 / 0.002 | 0.001 / 0.002 / 0.002 | 8.87 / 9.28 / 9.58 |
| Decode | 3.54 / 4.75 / 5.33 | 5.02 / 6.96 / 8.12 | 4.36 / 6.51 / 8.11 | 1.85 / 2.58 / 3.19 |
| End to end (present to client present) | 7.41 / 14.04 / 15.73 | 8.81 / 15.64 / 17.78 | 8.82 / 15.68 / 17.70 | 13.34 / 20.01 / 20.87 |

- **PyroWave's host total is mostly pacing.** Its packets go out over about 9 ms per frame, against microseconds for the AMF codecs, which burst on loopback. The receiver's assembly time matches (8.87 ms).
- **Network time is tens of microseconds and can be negative.** The host stamps "first sent" and "last sent" when the send call returns, and on loopback the receiver already has the packet by then.
- **The record costs nothing measurable.** H.264 with the record off and on, three runs each, from the host's 5 s `stream timings` lines (passing runs): present-to-send p99 11.85 ms off and 11.78 ms on; host mean 0.50 ms off and 0.57 ms on. Both are within run-to-run spread.
- **4 of 18 runs failed, and none of the failures came from the record.** h264-3, hevc-3, the first off run and the second on run failed. In each one, about 17 s in, Windows invalidated the default audio endpoint (`AUDCLNT_E_DEVICE_INVALIDATED`, 0x88890004) for both the host capture and the receiver's tone check. The failure hit runs with the record off as well as on. None was retried, there was no encoder hang, and the results above exclude them.
- **Not measured yet:** the same matrix to the NUC over the LAN, and phones over Wi-Fi.

## October 22: 3.0 alpha.2 gate

All runs were on the RX 7900 XT (AMD Software 26.9.2, Windows 26200) with isolated test hosts on their own profiles and harness ports. The installed service was idle throughout and never touched. Host builds: the Radeon endpoint build (`nj-e2e-host`, main after alpha.1) for the client runs, and the 3.0.0-alpha.2 package (main at `b758a8044` plus the version bump, after the full `build.ps1` gate) for the virtual-display crash case and the soak. Nightjar runs used builds of the 0.8.0 source.

**Couch loop.** Nightjar on a Retroid Pocket 6 over Wi-Fi, and on the Android emulator, did the following against a host on the LAN address:
- paired with the PIN, received Host control from the console API, and opened Home;
- played Desktop and opened Session and graphics;
- switched Anti-Lag on from the Radeon panel. A second paired client read it back as on, with the restore entry (original off, written on);
- chose End session. Anti-Lag was back to Off, with no restore pending, within 2 s.

A paired probe certificate got valid JSON from every GET endpoint (`/capabilities`, `/library`, `/session`, `/radeon`). `NativeApiVersion` was 1 over paired HTTPS and 0 over plain HTTP or unpaired.

**Crash restore.** The host was killed with `taskkill /F` mid-session, right after the Anti-Lag write. `radeon-journal.json` was on disk and named the setting. The next start logged `session_interrupted`, restored Anti-Lag to Off within 1 s of starting, and cleared the journal. The physical display's mode and position were unchanged.

On a virtual display (HEVC 2560×1440 at 120 fps, layout `extended`, as SYSTEM), the virtual monitor was gone 0.2 s after the kill. The primary 5120×1440 at 240 Hz display was identical before the stream, after the kill, after the restart and after the stop. This held both with the recovery helper killed alongside the host (`/T`) and with it surviving.

**Version cross-checks** (emulator):
- Nightjar 0.7.18 against the new host showed the classic grid, streamed Desktop at 60 fps and quit through Quit game. Nothing native-API related appeared in the log.
- Nightjar 0.8.0 against Rubylight 2.6.0 showed no Home screen, the classic grid and the classic stream menu with no Session and graphics page. It streamed and quit the same way.

**30-minute soak.** `e2e.py` streamed HEVC 1080p60 at 20 Mbps for 1,800 s from an isolated host on 127.0.0.1 to the independent receiver on the same PC, from the 5120×1440, 240 Hz primary monitor through WGC. No build ran during the soak. The desktop session, a browser and Defender were active.

| Measure | Result |
| --- | --- |
| Frames sent / decoded | 108,360 / 108,360, no decode failures |
| Steady fps (over 1,797 s) | 60.2 |
| Unique pictures/s | 60.2 |
| Arrival interval p99 / max | 21.3 / 32.4 ms, 1 long interval (alpha.1: 25.1 / 136.0 ms, 1,118) |
| Host mean (5 s windows, start / middle / end) | 0.62 / 0.51 / 0.47 ms |
| Encode mean, start / middle / end | 2.98 / 3.01 / 2.96 ms |
| Send interval max (worst 5 s window) | 23.0 ms |
| Picture age mean / p99 | 16.9 / 26.8 ms |
| Encoder hang | none |

The alpha.1 soak failed the receiver's gap check narrowly while the PC was loaded. This one passed every check.

Not covered: stock Moonlight on a phone.

## October 22: fence ring under GPU load

**Decision: the fence ring becomes the default for idd-push capture.** It was no worse than the keyed-mutex ring on every metric, and the loss of fresh pictures recorded on October 17 did not recur. The keyed-mutex ring stays available with `idd_push_fence_ring = false`.

Main's host package and main's display driver (stamped 2.6.0.7064 by `build.ps1 -Package`). RX 7900 XT, Radeon driver 32.0.32015.2008, Windows 26200. The setup matched the [alpha.1 gate](#october-22-30-alpha1-hardware-gate)'s virtual-display runs: a per-client virtual display in the extended layout, 2560×1440 at 120 Hz, `capture = idd-push`, HEVC at 80 Mbps, an isolated host run as SYSTEM, and the independent receiver with D3D11VA on the same PC. Streams lasted 30 seconds. The load was October 17's: `gpu_load` drawing a D3D11 shader at 2560×1440, 1,000 iterations over 200 draws, at 60 frames/s (9.3 ms of GPU time per frame). It held 60.0 frames/s in every run. The order was ABBA BAAB, with SDR and HDR streams in each round, so there were 4 runs per transport and range. The host log confirmed the ring each run used. "Fresh" counts distinct source pictures per second.

| Under load, mean (min–max of 4 runs) | Keyed-mutex SDR | Fence SDR | Keyed-mutex HDR | Fence HDR |
| --- | ---: | ---: | ---: | ---: |
| Fresh pictures/s | 120.03 (119.96–120.08) | 120.01 (120.00–120.04) | 119.69 (118.69–120.07) | 120.02 (120.00–120.04) |
| Picture age mean, ms | 14.40 (13.40–15.60) | 14.08 (13.65–14.41) | 14.61 (14.02–15.29) | 14.84 (14.59–15.11) |
| Picture age p99, ms | 19.34 (16.35–21.06) | 18.14 (16.18–20.41) | 21.33 (17.14–26.18) | 19.39 (16.94–21.59) |
| Frame interval p99, ms | 13.80 (12.22–14.97) | 11.22 (10.13–13.15) | 13.07 (10.96–15.20) | 12.35 (10.52–14.36) |
| Host mean, ms | 0.449 | 0.450 | 0.441 | 0.428 |

The fence ring was better or level everywhere except the HDR mean picture age. There it was 0.24 ms higher, well inside the keyed-mutex runs' own 1.28 ms spread. Every fence run delivered 120 fresh pictures/s, while one keyed-mutex HDR run fell to 118.7. Under this load the receiver's gap check failed in 6 of 8 keyed-mutex runs and 3 of 8 fence runs. That check flags interval spikes the load causes, and both transports hit it. There was no encoder hang and no new LiveKernelReports entry. These were short runs on one GPU, and a game's present cadence differs from `gpu_load`.

## October 22: 3.0 alpha.1 hardware gate

Main at `a14f1932b` (the AMD-only 3.0 line after decision 1a: physical displays, WGC, Desktop Duplication, D3D11 and both driver rings kept) against the published v2.6.0 package. Release builds only. RX 7900 XT, Radeon driver 32.0.32015.2008, Windows 11 25H2 (26200). Each stream ran a packaged host on an isolated profile on 127.0.0.1, with the independent moonlight-common-c receiver decoding with D3D11VA on the same PC. Streams were HEVC or AV1 at 2560×1440, 120 FPS and 80 Mbps for 20 seconds, with the moving strip and audio tone checks. Means are over runs, and each run's p95/p99 comes from its own steady frames. The installed 2.6.0 service stayed idle the whole time.

**Display driver.** Main's driver package (`rubylight-vdisplay` 0.2.0, DriverVer 0.2.0.7064, unsigned and signed on this PC by `install-local.ps1`) installed over 2.6.0.7037 with no restart. `ROOT\DISPLAY\0002` reported OK, and WUDFHost loaded `rubylight_vdisplay.dll`. After the virtual-display streams, the 2.6.0 driver went back for the installed 2.6.0 service, also with no restart.

**Physical display, ABBA BAAB.** The source was the 5120×1440, 240 Hz SDR primary monitor, letterboxed into the 1440p stream. Four runs per build and case. All 32 passed at 120 FPS with no decode failures.

| Case (main / 2.6.0) | Picture age mean ms | p95 ms | p99 ms | Host mean ms | Interval p99 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| WGC HEVC | 11.636 / 11.605 | 12.335 / 12.264 | 14.583 / 12.707 | 0.569 / 0.571 | 9.436 / 9.441 |
| WGC AV1 | 11.505 / 11.415 | 13.054 / 12.059 | 15.114 / 12.519 | 0.492 / 0.460 | 9.526 / 9.506 |
| Desktop Duplication HEVC | 10.786 / 10.875 | 12.223 / 12.433 | 13.677 / 14.105 | 0.502 / 0.504 | 9.363 / 9.355 |
| Desktop Duplication AV1 | 10.728 / 10.682 | 13.905 / 11.272 | 14.357 / 13.927 | 0.468 / 0.410 | 9.297 / 9.299 |

The tail is bimodal. In some runs, a few percent of frames arrive about 3.5 ms later, moving that run's p99 (or p95) from about 12 to about 15 ms. In this set it hit 10 of 16 main runs and 5 of 16 2.6.0 runs, and every WGC hit was on main. On AMD, nothing in the capture, conversion or encode path differs from 2.6.0: removing the NVIDIA/Intel branches leaves the same queue priority, compute copy and 4:2:0 shaders. So both WGC cases were repeated, ABBA BAAB again:

| WGC repeat (main / 2.6.0) | Picture age mean ms | p95 ms | p99 ms | Host mean ms | Interval p99 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| HEVC | 11.884 / 11.674 | 12.614 / 12.418 | 13.309 / 13.197 | 0.495 / 0.532 | 9.478 / 9.460 |
| AV1 | 11.646 / 11.662 | 12.376 / 12.394 | 12.756 / 12.642 | 0.381 / 0.380 | 9.569 / 9.550 |

No run in the repeat had the late mode. Neighbouring runs of the same build differed by up to 0.7 ms in mean picture age (HEVC 2.6.0: 12.04 and 11.35). Over both sets, main is within 0.15 ms of 2.6.0 in every mean, inside that run-to-run spread. The late mode follows the run, not the build: it is not a regression.

**Virtual display.** Per-client virtual display, extended layout, 2560×1440 at 120 Hz, `capture = idd-push`, run as SYSTEM in the signed-in session, main's 0.2.0 driver for both builds, ABBA. The host log confirmed `ring=KeyedMutex` (the default) and `ring=Fence` (`idd_push_fence_ring = true`), both zero-copy. All 8 passed, and the display layout matched before and after every run.

| Virtual display (main / 2.6.0), 2 runs each | Picture age mean ms | p95 ms | p99 ms | Host mean ms | Interval p99 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Keyed-mutex ring | 10.504 / 10.535 | 11.098 / 11.165 | 12.237 / 11.596 | 0.441 / 0.498 | 9.543 / 9.319 |
| Fence ring | 10.484 / 10.497 | 11.128 / 11.105 | 12.130 / 11.933 | 0.475 / 0.441 | 9.899 / 9.448 |

None of the 56 streams showed an encoder hang, and no new LiveKernelReports entry appeared. The 2.6.0 host also streamed on main's 0.2.0 driver with both rings. Not covered here: `check.ps1` against a tagged alpha, the 30-minute soak, stock Moonlight pairing, and the upgrade from 2.6.0.

**30-minute soak.** This ran the 3.0.0-alpha.1 package (main at `4f4d1af34` plus the version bump), built by `build.ps1 -Package` after the full gate passed. `e2e.py` streamed HEVC 1080p60 at 20 Mbps for 1,800 s from an isolated host on 127.0.0.1 to the independent receiver on the same PC. The source was the 5120×1440, 240 Hz primary monitor through WGC.

| Measure | Result |
| --- | --- |
| Frames sent / decoded | 107,814 / 107,814, no decode failures |
| Steady fps (over 1,790 s) | 60.1 |
| Unique pictures/s | 59.8 |
| Host mean (5 s windows, start / middle / end) | 0.43 ms |
| Encode mean, start / middle / end | 3.17 / 3.03 / 2.89 ms |
| Send interval max (worst 5 s window) | 32.5 ms |

- **No encoder hang**, and no `gpu_hang` in the log.
- **Present to send** stepped from about 6 ms to about 14 ms once, two minutes in. It then stayed flat until the end (12–14 ms), so nothing grew over the half hour.
- **The receiver's gap check failed narrowly.** It allows 1% of intervals above 1.5 frame periods (1,078 here) and a 50 ms maximum. The run had 1,118 such intervals and a 136 ms maximum at the receiver: arrival interval p99 25.1 ms, picture age p99 41.6 ms, picture age max 1,078 ms.
- **The PC was not quiet.** Cargo test runs and an Android build from parallel work ran during the soak, and the host's own send interval never exceeded 32.5 ms in any 5 s window. The gap check was written for 12 s runs on an idle PC.
- **The cadence result is therefore inconclusive.** A clean rerun is part of the alpha.2 gate, which repeats this one.

**Upgrade from 2.6.0.** The alpha.1 setup with `/quiet` ran over the installed 2.6.0 service while it was idle, and exited 0. Afterwards:
- the installed version read 3.0.0-alpha.1 and `ApolloService` ran;
- `/serverinfo` reported `RustHostVersion` 3.0.0-alpha.1;
- all 12 paired devices kept the same ids and names;
- in the profile folder, only `update-result.json` changed;
- the host log had no error lines, and the encoder capability probe completed.

**LAN pairing and stream from the NUC.** This stands in for stock Moonlight on another machine. The client was the Intel NUC on wired LAN, running the moonlight-common-c client and `rust/tests/interop.py` in its Docker test image. The host was an isolated alpha.1 profile on this PC, bound to its LAN address, port 48123. Pairing (PIN plus the RSA/AES proof) and the launch-permission checks passed. Then a 60 s stream of HEVC 1080p60 at 20 Mbps:

| Measure | Result |
| --- | --- |
| Frames sent / received / decoded | 3,592 / 3,597 / 3,597, every one with picture content, no decode failures |
| Steady fps | 60.1 |
| Arrival interval mean / p95 / p99 / max | 16.64 / 19.29 / 20.94 / 23.29 ms |
| Intervals above 1.5 frame periods | none |
| Host mean / p99 | 0.45 / 1.1 ms |
| Encode mean | 3.05 ms |
| Client decode mean | 7.16 ms |

No encoder hang. Stock Moonlight on a phone has not been run against alpha.1 yet.

## October 18: timer waits and idle capture

> **Reverted in 2.5.9 (October 19).** A field report ([#42](https://github.com/RamazanKara/Rubylight/issues/42)) showed the cost under load: PyroWave at 2772×1280, 144 fps on an RX 6600 XT averaged 35–55 fps and peaked near 58 on 2.5.6/2.5.7, against 78–118 average and 129–144 peak on 2.5.1 with the same game. Picture age p95 went from 1–2 ms to 6–11 ms. The host and WGC helper again use high process priority while streaming. Packet pacing does not bring back the 600 us yielding tail. Instead, `Timer::until_paced` arms the timer early by an eighth-weighted average of its measured lateness, capped at 1 ms. The pacer's accumulated deadline keeps the average rate exact.
>
> `pace_probe --load [--high]` (Ryzen 7 5800X3D, Windows 26200, release build, no socket) sends 260 KB frames at 144 fps and 500 Mbps beside one spinning normal-priority thread per logical CPU, two runs per wait. Paced: 144.1 fps, wire p95 3.56–3.60 ms, max ≤ 4.1 ms in every run. Old yielding tail: 104–143 fps, p95 5.2–10.2 ms. Plain timer: 133–144 fps. `GetThreadTimes` underreports yielding threads, so the probe's CPU column does not compare the spin variant fairly. Frame-claim waits and the idle capture policy below are unchanged from 2.5.6.

The packet and frame-claim waits now use the high-resolution Windows timer
throughout their deadline instead of yielding for the final 600 us. Ordinary
timer waits also no longer busy-wait their last 50 us. A timer-arm failure falls
back to a blocking wait that still accepts capture/recovery notifications.
Neither the host nor its WGC helper raises its whole process to high CPU
priority; media-thread scheduling is retained.

The release `timer_probe --compare` fixture compares the old wait implementations
from `ce3292ab` with the production timer in the same process. Measurements were
taken on October 18 on an AMD Ryzen 7 5800X3D, Windows 11 Pro build 26200. Both
variants used media-thread scheduling, normal process priority, a 1 ms timer
resolution request and the process power-throttling opt-out. There were two
three-second runs per case, reversing old/new order in the second pass; no build
or GPU benchmark ran during these measurements. This isolates the wait change,
not the effect of removing process-wide high priority.

| Synthetic workload | Thread cycles, million/second, old → timer | Mean deadline lateness, us, old → timer | Timer p95 lateness across runs |
| --- | ---: | ---: | ---: |
| Continuous 500 us packet batches | 3383.7 → 29.0 | 0.2 → 255.7 | 515–527 us |
| Frame claims at 120 fps | 127.3 → 3.07 | 4.8 → 281.8 | 501–513 us |
| Frame claims at 240 fps | 256.0 → 5.88 | 4.9 → 293.1 | 524–526 us |
| Static capture wait policy, no frames | 34.7 → 2.92 | 31.8 → 257.0 | 513–524 us |

The table averages the two runs. `QueryThreadCycleTime` reports execution
cycles; these are not converted into elapsed CPU milliseconds
or power consumption. `GetThreadTimes` also appears in the raw CSV, but reported
zero for some short busy-wait runs, so those zeroes are not treated as zero work.
For continuous packet pacing it reported 99.5–100% of one core before, versus
0–1.04% with the timer.

This is a tradeoff: substantially less CPU work with packet/frame waits waking
roughly 0.25–0.29 ms after their deadline on average. The worst candidate wake was
1.912 ms late. Frame cases completed all 360/720 deadlines per three seconds.
The continuous packet simulation completed 5902–5944 waits versus 6000 for the
old spin path; it uses the packet pacer's existing bounded 500 us catch-up
allowance. It does not prove unchanged UDP throughput, frame age, game smoothness
or client latency under load.

The idle capture policy keeps short polling during movement and pending GPU
handoffs. After 100 ms without a frame, existing helper/driver events may
interrupt a 10 ms wait; helpers still service control commands and health
checks within that bound. Direct WGC and Desktop Duplication keep their polling
path. In the no-frame wait simulation, three-second empty iterations fell from
5629/5651 to 388/386, including the initial active-poll window. This measures
the wait policy, not native capture delivery. The earlier
[loaded WGC notification experiment](#october-5-wgc-startup-and-notification-experiment)
is why active capture has not switched to pure notification waits. Pending
keyed-mutex acquisition still polls because an announcement can precede GPU
completion without another notification.

Regression coverage checks short deadlines, timer failure fallback, notification
interrupts, timer reuse and the idle/pending-frame boundary. The control worker
also reuses its session snapshot and borrows session IDs, while gamepad input
swaps and drains two retained queue buffers. Queue tests preserve release and
interruption behavior; these allocation changes have no separate CPU benchmark
claim here.

Reproduce without starting a host, changing displays or touching the installed
service:

```powershell
cargo build -p butterpollo-windows --example timer_probe --release --locked
& "$env:CARGO_TARGET_DIR\release\examples\timer_probe.exe" --compare > timer-comparison.csv
```

Use the SDK environment in [the build guide](../docs/building.md). The probe
prints run-level CPU time, cycle counts, wait counts and lateness distributions.

## October 17: loss-response verification

A supplied 2.5.1 log showed two distinct failures: Windows error `10055`
discarded a 44-packet video batch and two audio packets in one session; a later
4K60 HEVC session kept fast packet pacing through repeated unrecoverable ordinary
frames until an incomplete keyframe activated the fallback. The 60 Mbps client
budget correctly became a **46.988 Mbps encoder target** after FEC, audio and
control reserves. That target did not change when pacing fell from 800 to
93.976 Mbps.

The candidate, based on `ae1d8b91`, retries transient `10055` once after a short
backoff within the existing shared four-millisecond batch wait budget. It also
activates the existing loss guard after three distinct recent unrecoverable
frames within two seconds. Feedback available at sender selection moves output
to the dedicated sender before a slowed inline send. Manual pacing and the
optional adaptive controllers retain ownership. Healthy sends do not wait or
retry, and the encoder bitrate is unchanged.

### Native sender timing

One before/after pair used the existing `udp_pacing_probe` and independent Python
receiver on Windows loopback: 1,200 frames in 20 seconds, approximately 50 Mbps
UDP payload, 800 Mbps fixed packet pacing, and four frames eight times the normal
size. This measures sender timing, not physical network capacity.

| Measurement | Before | Candidate |
| --- | ---: | ---: |
| Received packets / complete frames | 92,084 / 1,200 | 92,084 / 1,200 |
| Missing packets / sender drops | 0 / 0 | 0 / 0 |
| Frame send mean / p99 | 0.497 / 0.615 ms | 0.538 / 0.684 ms |
| Maximum frame send | 8.211 ms | 8.154 ms |
| Send system calls / stalls | 2,444 / 0 ms | 2,444 / 0 ms |

The small mean/p99 increase in this single pair does not establish a speed gain
or a repeatable regression. Native loopback did not produce `10055`; injected
operation tests verify one successful retry, persistent failure, exhausted
budgets, fatal errors and no duplicate successful sends. A native timer test
also checks backoff when a socket still reports writable.

### Real encoding and decoding with synthetic loss feedback

Three requested ten-second streams used an RX 7900 XT, AMD driver
32.0.32015.2008, WGC on the unchanged physical 5120×1440 SDR 240 Hz display,
a moving 128-pixel timestamp strip, HEVC 1920×1080 at 60 FPS, a 60 Mbps client
budget and fixed 20% FEC. The separate Moonlight transport client received on
loopback and decoded with D3D11VA. Each host used an isolated profile; the
installed service stayed idle and display settings were unchanged.

For the feedback pair, the receiver sent three **synthetic** ordinary-frame
failure reports after three seconds, spaced at least 300 ms apart, while
continuing to receive and decode every frame. The healthy control sent none.
This checks control processing and sender handoff, not actual packet-loss
reduction or a remote client's scanout latency.

| Measurement | Before, synthetic reports | Candidate, synthetic reports | Candidate, healthy |
| --- | ---: | ---: | ---: |
| Received / decoded frames | 580 / 580 | 575 / 575 | 584 / 584 |
| Decode failures | 0 | 0 | 0 |
| Packet pacing | 800 Mbps throughout | 800 → 93.976 Mbps | 800 Mbps throughout |
| Encoder target | 46.988 Mbps | 46.988 Mbps | 46.988 Mbps |
| Dedicated sender handoff | No | Yes, after third report | No |
| Steady picture age mean / p99 | 12.94 / 23.18 ms | 16.10 / 24.29 ms | 12.28 / 15.21 ms |
| Steady receive interval mean | 16.640 ms | 16.640 ms | 16.635 ms |

Steady measurements exclude the first three seconds. Every steady decoded
picture had a distinct source timestamp. The deliberately reduced pacing adds
picture age on an uncongested loopback path; the purpose is to reduce bursts
after loss, not to lower latency on a healthy path. These short runs do not
justify changing the README's comparative performance claims.

The before host SHA-256 is
`a3320c1b97ae29cd5f453d0c51420d415f7a63dff9b60108c4e339a79ca14d19`;
the candidate is
`1fa3c6b18364d657ef26decb58dd1ab63c269b4dd9e7ea4e7063fbe2b378d6a8`.
The first fixture attempt received no frames because its client requested
1024-byte packets while its host override selected 1392. The matched runs above
used 1392 at both ends; the failed attempt is excluded from timing comparisons.

### Remaining large-frame limit

The reported RX 9070 XT output included a 1,206,396-byte encoded frame despite
an accepted AMF limit of approximately 195,783 bytes. Its 1,410,618 packetized
bytes need about 120.08 ms at 93.976 Mbps; the log measured 118.895 ms sending,
including 115.676 ms pacing, with only 0.0075 ms queue wait. The INFO log does
not identify that frame as an IDR. Earlier oversized outputs also exceeded
Moonlight's four-block FEC protection limit.

A separate 32-frame, 4K60 deterministic-noise AMF test on the RX 7900 XT used
46.988 Mbps and the same requested frame cap. VBR latency produced a 420,601-byte
startup IDR. Enabling HRD produced a byte-identical bitstream. CBR produced a
5,606,612-byte startup IDR and a 4,419,009-byte non-IDR maximum in this short
stress input. Both unique outputs decoded successfully. These results do not
support forcing HRD or CBR, so encoder defaults remain unchanged. They do not
reproduce or resolve the RX 9070 XT's midstream overshoot, or identify which
network hop caused the reported loss.

Validation: **1,189 workspace tests passed**, 68 ignored, zero failures;
strict workspace/all-target Clippy, Rust formatting and web checks passed.
The native summaries, raw logs, fixture sources, packet records and AMF
bitstreams are retained in `rubylight-bitrate-fix-20261017` in the local
artifact archive. The supplied log and diagnostic records are not published.

## October 17: fresh-picture throughput diagnosis

The capture wake-order fix in `1cbb0c9d` signals consumers after publishing the new
image and before releasing the previous GPU image. A CPU regression holds that old
image's destructor at a gate: notification failed before the reorder and succeeds
afterward. This removes an avoidable wake dependency; it does not establish a
hardware FPS gain or make GPU resource release asynchronous.

For a short diagnostic session, enable
`RUST_LOG=info,pacing=trace,stages=trace,idd_push=trace` before starting the host.
The following joins distinguish new capture objects from genuinely new pictures:

- IDD `image` records carry driver sequence/frame numbers and `host_acquired`.
  The exact same `host_acquired` value appears in `claim` and `encode returned`.
- `publish` and `claim` share `(source_id, capture_id)`. Their relative timestamp
  origins differ; use `claim - published` within the claim record for publication
  delay. `selected_is_latest` is a snapshot taken while emitting that record.
- `claim` and `encode returned` share `(stream_id, attempt)`.
  `submitted_presentation` joins the encoder's preserved timestamp to
  `(stream_id, presentation)` in `packetized picture`, which carries the wire frame
  number. A packetizer record proves assignment, not receipt; match it to receiver
  wire IDs and decoded source IDs before counting distinct delivered pictures.
- Each claim aggregates admission time in grid pacing, capture lookup/wait, fresh
  rate limiting, repeat waiting, encoder pressure and sender pressure. These are
  elapsed branch times including any output polling/sending, not pure sleeps.
  `encode_call_us` measures the synchronous encode call; codec completion can
  occur later. Failed or unsubmitted attempts must not be counted as delivered.

These traces are off by default. They add diagnostic work when enabled, so use
identical tracing for comparisons. No GPU image is cloned merely to inspect the
latest selection. Sequence gaps and repeated source pixels should be reported
separately: neither alone proves a network or encoder bottleneck.

### Short joined-trace check

Two 12-second requested runs used the RX 7900 XT, installed **2.5.1.6949** driver,
1440p120 HEVC Main10 HDR, the same graphics load and local receiver described below,
and identical TRACE settings. The candidate is based on `1cbb0c9d` with these
diagnostics, SHA-256
`a3320c1b97ae29cd5f453d0c51420d415f7a63dff9b60108c4e339a79ca14d19`.
Both used identical fixture DLLs; the unused PyroWave DLL came from 2.5.0.
The retained-final-frame driver fix was **not installed** in these measurements.
The matched pair deliberately set `minimum_fps_target=120`; the normal repeat
default for an unchanged desktop is 20. Repeats consume pacing credit, and at 120 Hz the
minimum claim spacing is 6.25 ms. In this fence run, 40 fresh claims immediately
after a repeat waited a mean 2.128 ms from publication (maximum 5.619 ms). This
matches the intentional spacing guard; it is not evidence of an unbounded loop.

| Measurement | Keyed | Fence |
| --- | ---: | ---: |
| Received / decoded, whole run | 1,390 / 1,390 | 1,427 / 1,427 |
| Steady seconds / received pictures | 9.553 / 1,148 | 9.794 / 1,185 |
| Distinct source pictures/s | 118.502 | 115.682 |
| Repeats using the same host image | 14 | 41 |
| Repeats in a newly captured host image | 1 | 10 |
| Published images replaced before selection, selected span | 0 | 16 |
| Fresh publication-to-claim mean / p99, microseconds | 62.6 / 836 | 550.0 / 3,879 |
| Prepared sender queue p99, microseconds | 20 | 21 |

All **2,333 steady received pictures** joined exactly through packetization,
encoder attempt, capture claim and driver image identity, including repeats.
Neither run entered encoder- or sender-pressure admission branches; socket writable
waits and packet drops were zero. Every consumed driver ring sequence was
consecutive. Fence driver presentation frame numbers skipped 17 values, while
keyed numbers were consecutive. Render-ledger cadence remained near 120 Hz;
that does not prove the compositor delivered 120 distinct surfaces per second.

Keyed selected every publication in its selected span. Fence also exhibited host
rate pacing and replacement of older publications before selection. This localizes
the observed loss of fresh pictures to acquisition/content delivery and, for the
fence case, admission timing. It does not identify the underlying compositor or
GPU scheduling cause, or prove that dropping the rate limit would help. Local
decoder p99 was also higher with fences (7.49 versus 2.32 ms), a same-GPU confounder.

One fence attempt stopped **before streaming** because the physical display changed
from 240 to 120 Hz during virtual-display setup. It restored the baseline; an
unchanged retry passed. All attempts restored display state and installed settings.
Raw evidence and exact join analysis are retained under
`D:/CodexArtifacts/rubylight-throughput-continue-20261017` and the matching
`throughput-trace-*` fixture runs. These short diagnostic runs do not isolate the
wake-order fix's benefit or establish a general performance improvement. The
experimental fence default remains off.

A final short fence control used the normal `minimum_fps_target=20`. All **1,107
steady pictures** joined exactly, every publication was selected, and host static
repeats disappeared. Nevertheless it delivered 103.284 distinct pictures/s and
112.544 wire pictures/s: 91 repeated source IDs arrived in newly captured images,
and driver presentation numbers skipped 49 values with no ring-sequence gaps.
Driver-image delivery was 112.515/s while the render ledger stayed at 120.006/s.
Encoder pressure remained zero; sender pressure appeared once for 4 microseconds,
with no packet drops or writable waits. This removes aggressive repeat filling
and publication replacement from that run's explanation. It does not establish
which compositor/GPU dependency delayed or repeated content, nor that changing
the repeat setting caused the lower fresh rate across these unsynchronized runs.
The installed configuration already uses the normal default and was unchanged.

## October 17: installed-driver fence comparison

**Decision: keep GPU fence transport experimental and off by default.** The installed-driver comparison confirms a shorter capture stage, but some loaded fence runs delivered substantially fewer fresh pictures. 2.5.1 ships recovery fixes for rejected fence transports, unsafe compute queues and missing producer completion during CPU readback. Existing capture methods and the keyed-mutex default remain available.

The tested host was commit `fa85daf84d74adc19594f7bb78909632cffb5632`, SHA-256 `e4ac552de4ac5f4cfd88620466ecdd246a86b7391d640faac7264db7c872d1ff`: 2.5.0 plus the recovery fixes. 2.5.1 retains this capture code; later changes cover version metadata, documentation and the web-test harness. It used an **RX 7900 XT** and the installed Rubylight **2.5.0.6947** display driver. An isolated host created an extended **2560×1440, 120 Hz** virtual display. Each run proved its actual fence/keyed transport, compute conversion and in-place capture. The installed service stayed idle.

### Method

Three short **keyed/fence/fence/keyed** groups covered SDR idle, SDR graphics load and HDR graphics load. HEVC used a 68,887 Kbps client request, 54,097 Kbps applied encoder target, fixed 20% FEC, 1,392-byte packets and 70.77 Mbps pacing. Adaptive controllers were disabled. A 128-pixel moving strip carried source frame IDs and QPC timestamps; the remainder of the virtual desktop was uncontrolled. The graphics workload was a D3D11 shader at 2560×1440, 1,000 iterations over 200 draws, capped at 60 workload frames/s with one completed workload frame outstanding.

An independent Moonlight receiver decoded on the same GPU. Each 16-second receiver run supplied **13.7–13.9 seconds of steady samples**, excluding the first two received seconds. Source/load ran for 20 seconds. One additional cancel/relaunch pair per format checked reconnects using the same paired identity. Picture age ends at local decode and barcode readback; it is not remote scanout or input-to-display latency.

### Per-run results

All **26,592/26,592 received frames decoded** across 14 successful cases and 16 connections. All **22,720 steady source timestamps** matched the source ledger. There were no decoder failures or missing received wire-frame numbers. HDR checks confirmed ten-bit pixels, BT.2020/PQ and valid HDR control. Every successful case restored the display state, installed settings and test-process privilege state.

| Group / order | Transport | Fresh source fps | Repeated source IDs | Picture age mean / p95 / p99, ms | Completion gap p99, ms |
| --- | --- | ---: | ---: | ---: | ---: |
| SDR idle A1 | Keyed | 120.010 | 0 | 14.476 / 15.381 / 15.863 | 9.447 |
| SDR idle B2 | Fence | 120.005 | 0 | 10.370 / 11.123 / 11.561 | 9.448 |
| SDR idle B3 | Fence | 120.001 | 0 | 15.326 / 16.039 / 16.608 | 9.443 |
| SDR idle A4 | Keyed | 120.006 | 0 | 15.189 / 16.042 / 16.598 | 9.655 |
| SDR load A1 | Keyed | 119.854 | 2 | 12.093 / 14.198 / 16.328 | 11.965 |
| SDR load B2 | Fence | 119.986 | 0 | 11.726 / 14.057 / 16.121 | 12.298 |
| SDR load B3b | Fence | 111.677 | 115 | 15.688 / 22.116 / 23.451 | 15.037 |
| SDR load A4 | Keyed | 119.940 | 1 | 15.589 / 16.892 / 17.372 | 9.624 |
| HDR load A1 | Keyed | 113.663 | 88 | 10.416 / 16.700 / 18.925 | 13.986 |
| HDR load B2 | Fence | 117.585 | 34 | 17.170 / 20.270 / 24.573 | 14.424 |
| HDR load B3 | Fence | 70.946 | 689 | 17.030 / 22.582 / 30.003 | 11.232 |
| HDR load A4 | Keyed | 115.824 | 58 | 11.590 / 12.436 / 21.258 | 10.614 |

The original SDR B3 attempt stopped on a physical-refresh assertion **before streaming** and restored the baseline. The failing value was not retained, so its cause is unknown. Instrumentation was added to retain active refresh values, and B3b passed the unchanged assertions. The original attempt is excluded from performance results. The four reconnect connections decoded 3,749/3,749 frames and restored the display between connections; one SDR reconnect also repeated source pictures. Full per-run and reconnect distributions are in [FENCE-VALIDATION.json](https://github.com/RamazanKara/Rubylight/releases/download/v2.5.1/FENCE-VALIDATION.json), with a [readable report](https://github.com/RamazanKara/Rubylight/releases/download/v2.5.1/FENCE-VALIDATION.md).

### Interpretation and limits

- **The capture stage is shorter.** Its mean was 139–153 µs for keyed versus 81–100 µs for fences across the comparison runs. This is capture presentation to acquisition, including elapsed waits; it does not measure CPU utilization. At idle, paired host presentation-to-last-send changes were only −0.043 ms and +0.002 ms. The first apparent 4.1 ms source-age gain did not repeat; source/display/stream phase is a major confounder.
- **Freshness regressed in some loaded runs.** SDR B3b repeated 115 source IDs versus one in keyed A4. HDR B3 repeated 689 versus 58 in keyed A4, delivering 70.946 versus 115.824 fresh pictures/s. Successful decoding and a smooth received-frame rate can conceal repeated pictures. Both transports showed some loaded variability.
- **The sender queue does not explain these runs.** Prepared-queue p99 stayed in the tens of microseconds, with no socket writable waits or sender drops. Driver drop/overwrite counters also stayed at zero. The worst HDR fence run had fewer driver acquisitions and a longer publication-to-encoder-claim interval while source rendering timestamps remained near 120 Hz. Current logs cannot establish whether composition, driver submission, transport scheduling, host admission or same-GPU decoding caused the difference.
- **Timing boundaries matter.** Host `capture` is presentation→acquisition, `hold` is acquisition→publication, and `claim` is publication→the stream thread taking the image for encoding. `claim` is not a ring CAS/mutex-call duration. Reused submissions may lack new capture-stage telemetry. The decoded source timestamp is independent of the capture presentation stamp, and a repeated source ID need not be a host static repeat. The Moonlight field keeps its Punktfunk-compatible packetizer boundary; sender queue and pacing remain excluded.
- **Coverage is limited.** Graphics workload averaged 60 fps, but matching that average does not synchronize composition or decoder phases. These short tests cover one GPU, driver and local receiver. They establish no remote scanout, natural-game quality, total CPU/power use, other-card compatibility or long-run stability gain. The automatic default would also affect additional reader/encoder combinations that this AMF comparison did not qualify.

Verification also passed **1,171 automated Rust tests**, strict Clippy, six serial native GPU regressions and 15 release-tool tests. The native checks cover pixel transfer, reader lifetime, delayed completion, missing producer completion before CPU mapping and replacement of a retired compute queue. No full installed-driver failure injection was performed.

## October 17: focused 2.5.0 comparisons

Short, isolated RX 7900 XT tests compared the 2.4.8 release with the 2.5.0 candidates. The installed service remained idle; its configuration, physical display mode, HDR state and RTSS settings were preserved. These comparisons cover recovery cadence, GPU scaling accuracy and a capture-transport component. They do not establish a ranking against other hosts or general input-to-display latency.

### Fresh input near the end of a recovery frame

Four 15-second runs in ABBA order used a physical WGC SDR source and HEVC stream both at 5120×1440, with a 119.88 FPS stream from a 240 Hz display. A controlled moving 128-pixel strip carried source IDs and QPC timestamps; the rest of the desktop was uncontrolled. The AMF target was 54,097 Kbps, pacing 70.77 Mbps, FEC fixed at 20%, packet size 1392 and the default maximum frame size two nominal frame budgets. Native resolution avoids a scaler change confounding this comparison. Independent Moonlight transport and D3D11VA decoding ran on the same PC.

| Measurement | 2.4.8 baseline | Recovery candidate |
| --- | ---: | ---: |
| Received and decoded frames | 3,506 / 3,506 | 3,435 / 3,435 |
| Forced IDRs / fresh following frames | 8 / 8 | 8 / 8 |
| IDR → next frame assembled, mean ms | 8.442 | 5.303 |
| IDR → next decoded barcode, mean ms | 7.938 | 4.960 |
| IDR → next host last-send, mean ms | 5.403 | 2.449 |
| Following picture age, mean ms | 17.394 | 17.770 |
| Following prepared queue, maximum ms | 0.014 | 0.011 |
| Steady picture age, mean / pooled p99 ms | 17.086 / 21.630 | 17.161 / 22.220 |
| Steady completion gap, pooled p99 / max ms | 12.060 / 20.541 | 12.081 / 21.171 |
| Decoder failures / socket drops / writable waits | 0 / 0 / 0 | 0 / 0 / 0 |

The recovery follow-up gap fell **3.139 ms (37.2%)**. Every candidate event (4.330–6.071 ms) was shorter than every baseline event (8.153–9.231 ms), and every following frame carried a newer source timestamp. All 6,941 decoded timestamps matched the source ledgers. Among 5,498 steady frames there were two repeated source timestamps in the baseline and one in the candidate. Overall picture age and tails did not consistently improve; this is a recovery-cadence result, not a general 37% latency reduction. Picture age includes local decoding and barcode readback. Remote scanout, Wi-Fi/WAN, HDR, game contention, other GPUs and other rates were not covered.

The change keeps the existing two-picture bound, packet order, FEC, pacing rate and Punktfunk-compatible host metric. A proven depth-one AMF pipeline waits for its IDR output before taking another input, then overlaps one fresh input with the end of paced transmission using observed encode/preparation time and actual socket progress. Unknown/deeper pipelines retain their priming, and socket stalls or drops restore the strict barrier.

Baseline source: `3685e7a04593cae9343921b434e91c2728bd9fd9`; packaged host SHA-256 `c0b3e3e23a24a21c436c64f982e4c7c37fec844fad60b4640ab0a05d4a17b150`. Recovery candidate SHA-256 `ab07fc8f45ecd8c54b6e223b981083767e9c52d86059c14d8d4fa84c48235192`; it precedes the final downscale-only specialization. The unchanged native-size shader path is separately covered below. Commands, settings, source hashes, timestamp ledgers and every joined event are retained under `D:\CodexArtifacts\rubylight-2.5.0-20261017\hardware`: `run-recovery.py`, `report-recovery.py`, `aggregate-recovery.py`, `recovery-native-abba.{md,json}`, `recovery-native-aggregate.json`, and `runs/recovery-native-{a1,b1,b2,a2}-1`.

### Antialiased GPU downscaling

On the RX 7900 XT, the color shader now integrates source-pixel coverage for reductions through 8×, reducing detail shimmer from skipped pixels. Native size and enlargement keep their previous paths; aligned 2× uses four bilinear samples, and exact 3× uses nine whole-pixel samples in fixed loops. Other supported reductions have a hard maximum of 9×9 source taps. Larger reductions retain the previous bilinear fallback. Existing aspect-fit, cursor composition, and HDR transfer behavior are preserved.

The final native check passed **66 cases on each of D3D11 pixel, D3D11 compute, and D3D12 compute**: three scrolling phases, ten-bit SDR/FP16 HDR, fractional and integer scaling, integer/fractional bars, and subpixel aspect-fill rounding. Every Y/UV sample was within one ten-bit code of an independent pixel-coverage reference. D3D11 compute luma matched exactly in 60/66 cases, with maximum RMS error 0.4923 codes in the remainder. All 24 native/enlargement/aligned-2×/>8× fallback controls were bit-identical to the former D3D11 compute shader.

The isolated D3D11 compute comparison used a 3840×2160 source, P010 output for both SDR and HDR, old/new/new/old order, 32 warm-ups and 128 GPU timestamps per arm. No compilers or other test GPU jobs ran during timing. Values below average the two arm means:

| Output / reduction | SDR old → new GPU time | HDR old → new GPU time |
| --- | ---: | ---: |
| 3840×2160 / native | 0.139 → 0.126 ms | 0.162 → 0.140 ms |
| 2560×1440 / 1.5× | 0.151 → 0.225 ms | 0.195 → 0.384 ms |
| 1920×1080 / 2× | 0.182 → 0.143 ms | 0.183 → 0.144 ms |
| 1280×720 / 3× | 0.138 → 0.244 ms | 0.139 → 0.248 ms |
| 480×270 / 8× | 0.068 → 0.056 ms | 0.059 → 0.057 ms |

Native timing stays within baseline arm variation; its lower mean is not a speedup claim. Exact 2× avoids unnecessary filter work. Correct 3× filtering costs about **0.11 ms** more, and the largest observed extra cost was **0.189 ms** at 1.5× HDR. This is conversion-only evidence on an idle RX 7900 XT; game load, other GPUs, capture, encoder, network, and display latency are outside this measurement.

A separate 30-frame synthetic scrolling-detail HEVC check used 3840×2160 BGRA input → 1280×720 SDR, 60 FPS, the same configured **8000 Kbps** target and AMF defaults. FFmpeg decoded **30/30 frames in both arms**. Against independent 3×3 source-pixel coverage, mean/minimum frame luma PSNR changed from **8.81/8.79 dB to 50.99/37.69 dB**. Achieved encoded payload equivalent was **5239.792 → 4983.280 Kbps**, and the initial IDR was **61,883 → 45,894 bytes**. The large PSNR gain demonstrates removal of intentional aliasing in stripes, checkerboards, diagonal strokes, and a gradient; it is not a natural-game or perceptual-quality score. The configured target was identical, but achieved bitrate was not, and payload rate excludes FEC/network overhead. No end-to-end or host-market comparison is implied.

Evidence is retained under `D:\CodexArtifacts\rubylight-2.5.0-20261017`: `downscale-coverage-v3.{json,log}`, `downscale-timing-v3.{json,log}`, `quality/baseline-source-size-verified/report.json`, `quality/candidate-v3/report.json`, the corresponding `encoded.hevc` files, and `quality-encode.py`. `downscale-results.md` also records the initial 2× regression and its correction. Reproduction entry points are the ignored `gpu_downscale_coverage_matches_independent_reference` and `gpu_downscale_timing` tests, plus the `amf_quality` example driven by `quality-encode.py`.

### Experimental GPU fence transport

The optional `idd_push_fence_ring` path keeps the D3D11 display driver and adds D3D12-owned shared textures with written/released GPU fences. The keyed-mutex transport remains the default. All existing capture and encoder options remain available.

One ABBA run per format used 2560×1440, nominal 120 Hz, eight warm-up frames and 64 measured frames per round: 128 per transport/format, 7.10 seconds total. Two independent D3D11 devices in the same process simulated producer and host, with conversion on D3D12. The inputs alternated two preuploaded flat-color textures; the timings end at completed conversion.

| Format | CPU claim, keyed → fence | Copy through completed conversion, keyed → fence |
| --- | ---: | ---: |
| SDR BGRA8 | 31.627 → 1.745 µs | 658.411 → 592.906 µs |
| HDR RGBA16F | 32.471 → 1.767 µs | 756.199 → 697.567 µs |

These are pooled means of equally sized rounds. Claim overhead consistently fell, but completed timing was noisy: adjacent SDR pairs saved 98.439 and 32.571 µs; HDR saved 155.152 µs in one pair and regressed 37.888 µs in the other. The combined completed-component saving was about **0.062 ms**. This does not establish consistent tail or end-to-end improvement.

Three native checks passed SDR/HDR pixels, copy/in-place reads, multiple readers, images surviving ring replacement, and delayed GPU release preventing premature reuse. The driver passed MSVC Clippy and DLL compile/link. This fixture did not run an installed IddCx driver, WUDFHost, DWM, an encoder, network or receiver, and did not test native IddCx D3D12 swap-chain support. It establishes no picture-quality gain or long-term stability result.

The option stays **experimental and disabled by default**, with capability checks, initialization fallback and failed-ring quarantine. Installed-driver and receiver evidence is required before changing that default. Reproduce the component checks with the explicitly ignored `capture::idd_push::fence_tests` tests. Raw logs and build/source provenance are retained in `D:\CodexArtifacts\rubylight-2.5.0-20261017`: `fence-timing.log`, `fence-pixels.log`, `fence-lifetime.log`, `fence-delayed-gpu.log`, `fence-results.md`, `driver-clippy.log`, `driver-build.log` and `driver-source-hashes.json`.

## October 17: Punktfunk host timing and paced keyframes

The change released in 2.4.8 matches Punktfunk's GameStream software-encoder timer: the capture/encode output tick through entry into a separate packetizer. Prepared packets already contain the field before waiting for the paced sender. Asynchronous output uses the tick that polls it; encoder duration and present-to-last-send remain separate measurements. Packetizer backlog can still count. The wire field truncates microseconds to tenths of a millisecond and saturates at 65535. See the [reference source and definitions](../docs/performance.md#comparing-host-latency-between-hosts). Earlier sections retain the timing definitions of their measured builds.

Compared installed **2.4.7** (`09bcf3926188f79ca64e729c7b40b34f377be031231f4f90cd4d40fcfa52e6b8`) with the optimized candidate (`2fec83854f31196b696cbd56ad4b76009a51644b3699dc4ab154eec24bb7dab3`) on the RX 7900 XT. Both ran as separate processes on isolated profiles bound to 127.0.0.1, base port 48523. The installed service stayed running and idle; its profile, RTSS, physical display mode and HDR state were unchanged.

The source was the current **5120×1440 at 240 Hz SDR physical WGC display**, scaled/letterboxed to **HEVC 3840×2160 at 119.88 FPS**, AMF VBR latency at 54,097 Kbps. A static desktop was re-encoded with `minimum_fps_target=120`; no controlled motion or watermark was shown. Fixed pacing was 70.77 Mbps, FEC 5%, minimum parity 2, packet size 1392. Adaptive bitrate/pacing/FEC and lost-keyframe pacing were disabled in these isolated fixtures. Each build ran twice for 15 seconds, with four requested IDRs 1.5 seconds apart. An independent Moonlight common client at `2600beaf13f18bfa43453609cf5e3b84a4227760` used D3D11VA decoding on the same GPU. The fixture copy of `rust/tests/moonlight_client.c` used packet size 1392 to match the host.

| After 3-second receiver warmup | 2.4.7 round 1 | 2.4.7 round 2 | Candidate round 1 | Candidate round 2 |
| --- | ---: | ---: | ---: | ---: |
| Receiver FPS | 120.303 | 120.201 | 119.856 | 119.640 |
| Moonlight host mean, ms | 5.812 | 5.875 | 0.451 | 0.470 |
| Moonlight host p99, ms | 6.6 | 6.8 | 1.7 | 1.6 |
| Moonlight host maximum, ms | 7.6 | 40.9 | 2.5 | 2.5 |
| Tracked encode mean, ms | 5.606 | 5.602 | 5.565 | 5.531 |
| Tracked encode p99, ms | 6.385 | 6.565 | 6.369 | 6.363 |
| Tracked present-to-last-send mean, ms | 9.594 | 9.225 | 7.829 | 7.209 |
| Tracked present-to-last-send p99, ms | 17.476 | 16.725 | 16.951 | 16.215 |
| Maximum receive interval, ms | 37.189 | 41.820 | 37.156 | 46.549 |

Every received frame decoded: baseline 1782/1785 frames, candidate 1763/1777, and all four requested IDRs completed in each run. There were zero host packet drops or socket writable waits. Tracked stages cover new pictures within the steady receiver window; repeated encodes lack those records. The desktop contents and capture samples differ between rounds, and the candidate's largest IDRs were larger (up to about 338 KB). Candidate throughput was about 0.4–0.6 FPS lower, with both builds near the 119.88 target. These runs establish decoding and timing-boundary behavior; they do not establish an end-to-end speedup, native 4K/HDR performance, PyroWave hardware performance, or the remote game client's network capacity.

The baseline directly reproduced the queue spike: round 2 frame 701 was a **313,713-byte IDR**, with **36.959 ms deliberate pacing**, **1.583 ms socket calls**, no writable waits and no drops. Frame 702 then waited **33.791 ms** in the old combined packetizer/send queue, and its Moonlight host field reached **40.9 ms**. Candidate keyframes still took roughly **25–41 ms** of deliberate pacing. Separating the timer does not remove the wire serialization time. The new `send_queue` stage and throttled `paced video frame delayed` log expose this delay independently.

The candidate also proves the boundary directly outside the warmup table: after round 2's startup IDR, frame 2 waited **9.939 ms in the prepared-send queue** while its received host field stayed **0.3 ms** (packetizer queue 0.014 ms). Later, forced IDR frame 698 used **38.161 ms pacing plus 1.625 ms socket work**, while its host field was **0.5 ms**. All four successful runs used identical profile settings; candidate runtime DLL hashes matched the baseline.

The preceding live RetroBat investigation found an adaptive-bitrate clamp reducing effective pacing from 800 Mbps to 83.482 Mbps and then 70.7695 Mbps, even with the separate adaptive packet-pacing controller disabled. At 54,097 Kbps, average packetized demand was 62.1995 Mbps, leaving little room for large VBR frames. The initial reduction followed two unrecoverable FEC reports; the later reduction matches the rising-control-RTT path by source inference. Quiet scenes can reset the controller's eight busy/stable seconds needed for recovery. An isolated production-pacer loopback probe also delivered all 52,080 datagrams without a writable wait or drop: a 225,632-byte frame took 27.623 ms at 70.7695 Mbps versus 1.899 ms at 800 Mbps. Pacing limits, loss safeguards and encoder frame-size settings were not changed by this fix.

Validation includes workspace tests, all-target Clippy with warnings denied, formatting, the web type check, and regressions for the exact wire quantization, encrypted/plaintext timing frozen across a blocked sender, keyframe skips with frame/IV continuity, shutdown while a handoff is blocked, PyroWave latest-frame replacement, and the split full-path timing. The reusable fixture, its dependency and binary hashes, raw logs/receiver CSVs, display diagnostics and comparison summaries are retained under `D:\CodexArtifacts\rubylight-host-metric-20261017\queue\hardware` (`run.py`, `analyze.py`, `README.md`, `provenance.json`, `baseline-correct-*.json`, `candidate-*.json`). Invalid packet-size/Python fixture setup attempts are retained separately and excluded from the comparison.

### Existing AMF frame-size limit at the observed pace

A follow-up used the same retained candidate (`2fec83854f31196b696cbd56ad4b76009a51644b3699dc4ab154eec24bb7dab3`) and isolated hardware fixture to compare the previous driver-default setting `amd_max_frame_size=0` with `2`. The order was **default A, cap2 A, cap2 B, default B**, each 15 seconds with four forced recovery IDRs. The physical SDR WGC source, HEVC 3840×2160 at 119.88 FPS, 54,097 Kbps encoder target, 70.77 Mbps pacing, packet size 1392 and independent same-GPU decoder were unchanged. **FEC was fixed at 20% on every frame**, rather than the preceding fixture's 5%; the client request became 68,887 Kbps to retain the same encoder target after reservations. Live recovery IDRs keep the configured 20% protection, but ordinary live frames can adapt lower. All adaptive controllers remained disabled in the isolated profiles.

AMF readback confirmed `HevcMaxAUSize=0` versus **902,519 bits** (about 112,815 encoded bytes, twice the nominal frame budget). All four profiles were identical apart from this setting. During these tests the installed service stayed idle, and its profile, physical display/HDR and RTSS were unchanged.

| Measurement | Default A / B | Cap2 A / B |
| --- | ---: | ---: |
| Forced-IDR payload range, bytes | 208,507–332,237 / 258,889–400,338 | 113,726–115,278 / 113,177–115,482 |
| Forced-IDR accounted wire range, bytes | 269,742–430,408 / 334,598–515,900 | 147,400–148,874 / 147,400–148,874 |
| Forced-IDR deliberate pacing range, ms | 28.404–45.750 / 34.968–54.494 | 14.846–14.897 / 14.846–15.582 |
| Maximum following-frame prepared queue, ms | 43.307 / 0.013 | 10.562 / 10.941 |
| Mean request-to-decoded IDR, ms | 44.960 / 58.077 | 26.600 / 25.082 |
| Receiver FPS after warmup | 119.707 / 119.272 | 120.337 / 120.273 |
| Maximum receive interval after warmup, ms | 45.722 / 58.452 | 25.830 / 21.230 |
| Actual tracked encode mean after warmup, ms | 5.529 / 5.542 | 5.568 / 5.588 |

The IDR rows include all four forced requests, including the first just before the three-second receiver warmup. Wire bytes are the host's accounting of padded/FEC packets and transport overhead; payload bytes are actual encoder output, not the receiver's padded byte count. Default A frame 337 spent **45.750 ms pacing plus 2.118 ms in socket calls**, then frame 338 waited **43.307 ms** in the prepared queue. Cap2 A frame 340 spent **14.897 ms pacing plus 0.672 ms socket work**, followed by **10.562 ms** queueing. Other next-frame queues were nearly zero when the next frame became ready after transmission ended; queue time alone does not represent recovery delay.

All **7,112 received frames decoded**, all **16 requested IDRs completed**, and sender traces had **zero packet drops or socket writable waits**. The cap reduced real recovery transmission time at the same pacing rate. It remained a soft limit: capped IDRs exceeded the requested bytes by up to about 2.4%. This short, mostly static SDR fixture with uncontrolled changing content does **not** establish native 4K HDR game quality, rendered-picture latency or remote path capacity. The limit applies to other large encoded frames too and can reduce visual quality. The experiment itself changed no installed settings or global defaults. The subsequent 2.4.8 policy uses `2` by default, with `0` available for quality/compatibility comparisons.

Full commands, binary/client/runtime/script and evidence hashes, actual settings, all 16 joined IDR/next-frame records, distributions, raw receiver CSVs and before/after display diagnostics are retained in the same artifact directory: `frame-size-comparison.md`, `frame-size-comparison.json`, `summarize-frame-size.py`, `size-default-detail.json`, `size-cap2-detail.json` and `runs/size-*`. The original metric fixture is preserved byte-for-byte as `run-metric.py`; `run.py` now accepts `--fec 20 --max-frame-size 0|2 --client-bitrate 68887` for this comparison.

Two additional 12-second compatibility checks used explicit `amd_max_frame_size=2` on the same retained candidate: **H.264 and AV1 at 1920×1080, 60 FPS**, with the same physical SDR source, 30,988 Kbps actual encoder target, fixed 70.77 Mbps pacing and 20% FEC. AMF read back **`MaxAUSize=1032933`** for H.264 and **`Av1MaxCompressedFrameSize=1032933`** for AV1. Independent D3D11VA decoding completed **712/712** and **716/716** frames respectively, **4/4 forced IDRs each**, zero decoder failures, and zero socket writable waits or packet drops. Steady receiver rates were **60.347 / 60.257 FPS**. GPU-hang guards did not trigger; the installed host stayed idle and display/HDR/RTSS were unchanged during the runs. These explicit-value checks cover codec compatibility on this RX 7900 XT, not unsupported-driver fallback or game/HDR visual quality. Commands, hashes, settings, logs and all eight IDR records are retained as `run-codec-cap.py`, `codec-cap2-report.md`, `codec-cap2-report.json`, `codec-cap2-detail.json` and `runs/codec-cap2-*` in the same artifact directory.

## Initial measurement environment

Measured locally on 2026-10-01: Ryzen 7 5800X3D (8 cores/16 threads), RX 7900 XT, AMD driver 32.0.31041.1004, Windows x64, Rust 1.98.1 release builds. The initial HEVC/AV1 runs captured a 1968×2184 HDR desktop. The later 2.0 candidate runs captured a 2560×1440 SDR desktop and converted/scaled it to the requested format; physical HDR remained disabled. Neither source establishes native 4K capture performance. Hosts ran on loopback with isolated configurations and display changes disabled. The installed production service was preserved; it was stopped during the 2.0 candidate tests.

The measured reason to switch is video FEC generation: the Rust implementation
is 1.26–1.40× faster than the original C++ implementation on representative
video blocks, using 21–29% less CPU time for identical parity bytes.
GPU-resident HDR processing, bounded texture/encoder queues and a Rust-rendered
console are additional implementation benefits.

## Capture and encoder wakeups on Windows

On 2026-10-02, the installed candidate's short condition-variable timeouts were measured on the same Windows machine. A requested 250 µs encoder poll could sleep for a scheduler tick. Capture consumers now wait on their own unnamed event together with the worker's high-resolution timer. A capture arriving before the wait remains signaled; one client resetting its event cannot consume another client's notification. The wait does not spin or change the system timer resolution. Windows documents this combination in [waitable timers](https://learn.microsoft.com/en-us/windows/win32/sync/waitable-timer-objects) and [wait functions](https://learn.microsoft.com/en-us/windows/win32/sync/wait-functions).

| Requested wait | Condition-variable mean / p95 | Capture event + timer mean / p95 |
| --- | ---: | ---: |
| 250 µs | 15.293 / 16.309 ms | 0.615 / 1.010 ms |
| 500 µs | 15.206 / 16.289 ms | 0.971 / 1.052 ms |
| 16.667 ms | 30.775 / 32.101 ms | 16.798 / 17.010 ms |

Each case uses 40 actual waits in the same release process. These measurements establish lower timeout overshoot on this Windows installation, not a 25× whole-stream speedup. Reproduce with `cargo run -p butterpollo-windows --example wait_performance --release --locked` from `rust` in the SDK environment used for the build.

An unchanged desktop image also no longer consumes the next encode slot. Fresh content arriving after a waiting deadline can be submitted immediately, while encoding submissions retain the configured cadence. Resuming after a longer static interval starts a new cadence without a catch-up burst. Tests cover scheduler overshoot, static resumes, capture arriving before a wait, independent capture consumers and timer reuse.

These October 2 session API and five-second host log samples reported capture-to-packetization host processing separately from encoder latency. Current native AMF streams instead reconstruct a claim timestamp from the encoder's measured duration: before capacity checking and GPU conversion submission through collection of the matching `QueryOutput` result. That duration includes unfinished capture-copy and conversion dependencies, but excludes subsequent output-buffer copying and packet sending. Host processing runs from the reconstructed claim to the inline sender's sample or the separate sender's dequeue; the latter includes sender-capacity and queue waiting. The current loopback runs used inline sending. Only the frame-header value is rounded to 0.1 ms; capture age is separate. Neither definition includes client scanout or input latency; compare only measurements using the same definition. Full-stream idle-desktop comparisons are recorded below; a dynamic game and the customer's client remain separate acceptance checks.

Independent encrypted Moonlight decoding captured the existing native 1968×2184 HDR display, with HEVC Main10, a requested 40 Mbps, 20-second requested runs and the default 20 FPS static repeat target. The actual negotiated encoder bitrate was 30,988 kbps. Display changes were disabled in the isolated loopback profiles; the installed service stayed running without a client during these comparisons. Both builds decoded all four runs without a reported failure.

| Requested stream rate | Installed candidate host mean / p95 | Revised host mean / p95 |
| --- | ---: | ---: |
| 60 FPS | 4.548 / 4.700 ms | 4.183 / 4.200 ms |
| 120 FPS | 4.187 / 4.200 ms | 4.120 / 4.200 ms |

Measured host steady rates rose from 16.83/18.15 FPS to approximately 20 FPS
for the static desktop.

## Customer regression checks on 2026-10-02

The connected phone reported missing audio, sluggish mouse input and approximately 11 ms average / 20 ms maximum host processing at 1968×2184/120 AV1 HDR. Its actual host log confirmed WGC was being opened even though the requested backend was `ddx`, and virtual speaker routing failed with `0x88890008`. The revised build recognizes the old capture alias, composes DDX's separately supplied cursor in the GPU color pass, and restores the previous PCM/float speaker format choices, including valid 24-bit samples in a 32-bit container. Input polling uses the same high-resolution timer as capture.

A same-process input wait comparison (40 waits per case) measured the previous requested 1 ms thread sleep at 1.534 ms mean / 1.576 ms p95, versus 1.022 / 1.030 ms with the timer. This is a polling-wait improvement, not a measured mouse-to-display latency reduction.

The service-context fixture runs as LocalSystem in the signed-in desktop session, uses a separate loopback profile and cancels if the phone connects to the installed host. It creates an actual 1968×2184 HDR virtual display and streams 120 FPS with an 80 Mbps requested bitrate. Desktop Duplication is explicitly logged. HEVC strict independent decoding passes with zero failures, including nonzero audio from a quiet 960 Hz tone rendered to Steam Streaming Speakers. WASAPI captures the tone; encrypted transport and independent Opus decoding produce a peak of 0.053 and RMS of 0.032. Teardown restores the audio journal. This covers real routing rather than merely successful decoding of silent packets.

| Actual virtual-display stream | Steady host FPS | Steady host mean / p95 | Steady maximum |
| --- | ---: | ---: | ---: |
| HEVC HDR 1968×2184 | 119.72 | 3.735 / 3.831 ms | 4.653 ms |
| HEVC HDR, next five-second sample | 119.72 | 3.768 / 3.940 ms | 6.738 ms |
| AV1 HDR 1968×2184, final five-second sample | 119.67 | 3.192 / 3.243 ms | 3.470 ms |

The HEVC client's whole-run mean is 5.733 ms and p95 is 3.900 ms: startup outliers raise the mean. Its 99.35 decoded FPS includes a two-second first-frame delay in the requested 12-second run, so that total is not a steady streaming rate. The AV1 run is **not an interoperability pass**: it decodes 1984×2186 rather than the requested 1968×2184, and every decoded frame fails the unchanged dimension check. Its first steady sample has 10.439 ms p95; the later sample above is lower. AMD's [AV1 alignment contract](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/wiki/AV1-Encoder#av1-specific-api) permits padded output for unaligned dimensions. The host does not silently change codec, resolution or quality to pass this check. Both privileged runs log a Windows layout-restoration warning; their separate recovery helpers subsequently clear the journals and restore the physical-only desktop. This is not a clean immediate-restoration acceptance pass.

The paced AV1 encoder probe, with actual DDX capture kept active, returns 120
FPS and 3.216 ms mean / 3.614 ms p95 from submission to observed output. A
repeat of the earlier WGC-backed probe also measures about 3.3 ms; these probes
show no encoder-only advantage from changing capture backend.

Reproduce the active-capture component probe with `butterpollo-performance.exe --capture ddx --live-capture --encoder amf --codec av1 --hdr --width 1968 --height 2184 --fps 120 --bitrate 80000 --seconds 6 --paced`. The independent stream fixture accepts `BUTTERPOLLO_TEST_MATCH_DISPLAY=1` to request the matching display mode and `BUTTERPOLLO_TEST_AUDIO_TONE=1` to require a nonzero decoded tone. The latter requires a separate renderer such as `windows/examples/audio_probe.rs`; setting the variable alone fails a silent stream. Privileged virtual displays require the installed service's security context; administrator elevation alone is not sufficient for this driver's access policy. The optional `session_command` example reproduces that context from a test-owned LocalSystem task without changing driver permissions.

## Video packet processing after the first 2.0 candidate

Compared with Rust candidate `118d0ef133ae2d63898fb61512d25b6253d9c79b`, the revised packet path uses **11–16% less CPU time for encrypted frames** in these workloads. It reserves the encryption envelope in the final shard allocation, excludes that envelope from FEC, reuses AES-GCM setup within each frame and encrypts the shard in place. Source data goes directly into its shards, avoiding a full packed-frame copy. Windows UDP batches also use a bounded stack buffer for their descriptors.

| Data shards, 20% FEC | Encryption | Previous Rust median | Revised median | CPU time saved |
| --- | --- | ---: | ---: | ---: |
| 32 | Off | 8.87 µs | 7.57 µs | 15% |
| 32 | AES-GCM | 47.73 µs | 40.12 µs | 16% |
| 192 | Off | 160.61 µs | 151.44 µs | 6% |
| 192 | AES-GCM | 393.96 µs | 344.61 µs | 13% |
| 576 | Off | 492.40 µs | 457.43 µs | 7% |
| 576 | AES-GCM | 1185.92 µs | 1045.23 µs | 12% |

Each case uses seven 250 ms rounds on the same Ryzen/Rust release environment,
with prebuilt identical payloads of 44,024, 264,184 and 792,568 bytes. Timing
includes header construction, allocations/copies, FEC, optional encryption and
packet release; it excludes capture, encoding and UDP sending. Both builds
produce the same first-frame SHA-256, packet counts and wire byte totals in all
six cases. Tests separately compare every encrypted shard against independent
per-packet sealing through FEC, partial tails and sequence/frame/nonce
boundaries.

The package includes `butterpollo-video-packet-performance.exe`. Run it without arguments to print the workloads, timing samples and packet fingerprints. The source is `core/examples/video_packet_performance.rs`; use that same harness in the previous checkout when reproducing the comparison.

## October 2 latency work

The candidate removes synchronous display renewal and monitor enumeration from
the lock used by the capture and encoder workers. Those workers now read a
published output/generation pair; maintenance publishes a new identity after
native work finishes. A twenty-sample, read-only probe on the physical display
measured monitor enumeration at 1.558 ms mean, 2.605 ms p95 and 4.333 ms maximum.
This establishes a potentially expensive dependency in the old frame path,
not a measured full-stream improvement from removing it. HDR metadata reads
were negligible in this probe and remain on their existing polling schedule.

Same-size GPU color conversion now loads each source pixel once, preserving
the existing resize, HDR transfer, chroma and cursor behavior. At 1968×2184
FP16-to-P010, 64 D3D11 timestamp samples after 16 warmups measured 0.335 ms mean
and 0.337 ms p95 before, versus 0.170 ms mean and 0.175 ms p95 after. This is an
approximately 49% reduction in this GPU component's elapsed time. It excludes
capture, encoding, transport and decoding. The opt-in
`gpu_color::tests::gpu_conversion_timing` test reproduces the measurement.

Pointer-only Desktop Duplication updates reuse immutable owned desktop pixels
and update the detached cursor snapshot. A missed new desktop copy invalidates
the cache, preventing stale pixels after texture-pool exhaustion. Native
readback checks cover retained images, the full bounded pool and recovery.
Per-device GPU priority and maximum-frame-latency hints now match the previous
host, independently of whether privileged process scheduling is available.

Diagnostics add p99, an explicitly labelled capture-age estimate, and frame
send-completion intervals. The original source timestamps and Moonlight
processing durations remain intact.

A normal-user isolated HEVC Main10 stream passed encrypted pairing/permissions,
exact 1968×2184 output, BT.2020/PQ, and independent Opus decoding of a quiet
known tone. It requested 120 fps and 80 Mbps. After a five-second warmup, 900
frames delivered at 119.993 fps with no intervals above 1.5 frame periods.
Host processing mean/p95/p99/max was 5.517/5.9/6.3/6.4 ms. Arrival intervals had
9.198 ms p99 and 9.375 ms maximum. This test captured an unchanged 2560×1440
SDR desktop and converted/scaled it; it excludes native HDR motion acceptance
and is not a whole-host comparison with Vibepollo.

The independent AV1 geometry gate still fails on this RX 7900 XT. Requested
1920×1080, 1968×2184 and 2184×1968 decode as 1920×1082, 1984×2186 and 2240×1968,
respectively, in both SDR/HDR with tested alignment modes 3 (`NO_RESTRICTIONS`)
and 4 (`8X2_ONLY`). Requested component dimensions and alignment read back correctly, but
bitstream traces contain enlarged dimensions without a render-size correction.
The same customer-size failure occurs in the current C++ baseline. Do not
count these as exact-resolution passes or change the strict decoder gate.
[AMD's corresponding bug report](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/issues/423).

Dynamic loopback tests additionally render a changing barcode containing source
sequence/QPC values, then timestamp independent low-delay decoding. This
picture-age measurement includes rendering, DWM, capture, encoding, loopback
and software decode; it excludes remote display scanout. Source refresh must be
measured and matched, not inferred from a requested virtual-display mode. The
attempted 240 Hz C++ fixture actually presented at 120 Hz; comparisons with the
240 Hz Rust fixture are therefore not accepted. The next elevated matched
benchmark launch was rejected by automatic approval review.

## October 4: capture copies and conversion beside a game

On an RX 7900 XT (AMF 1.5.2), AV1 HDR at 1968×2184 already encodes at the
VCN's floor: 2.8-3.3 ms from submission to bitstream when idle. Tiles,
multi-VCN, pre-encode, CDEF, AQ, the lowest-latency mode and QueryTimeout 0
measured no gain; the driver already applies them under ultra-low latency.
The cost is elsewhere: `examples/gpu_load.rs` draws a game-like load
(≈5.6 ms GPU frames), and beside it every D3D11 step waits behind the game on
the graphics engine (conversion 7.2 ms instead of 0.9 ms, a plain copy 8.4 ms
instead of 0.75 ms). D3D11 GPU priorities, process scheduling classes and a
high-priority D3D12 direct queue did not change that. A D3D12 compute queue
did the same conversion in 0.22-0.29 ms under the load (`d3d12_probe`,
`copy_probe`).

Captured frames are now copied into shared textures on one compute queue and
converted on another, and AMF encodes from D3D12 (`windows/src/compute.rs`).
Two synchronisation details decided correctness:

- Desktop Duplication returns a frame while DWM's copy into it may still be
  running; the D3D11 device waits for it through the keyed mutex. A compute
  copy without that wait differed from the D3D11 copy in 159 of 788 frames
  idle and 316 of 325 beside the load (`ddx_sync_probe verify-nosync`). The
  D3D11 context signals a shared fence after acquiring, the copy waits for it
  on the GPU, and the D3D11 context waits for the copy before the frame is
  released: 0 of 1161 idle and 0 of 267 under load differed. A D3D11 fence
  signal holding no frame completes in 0.016 ms beside the load; holding a
  frame it completes when DWM's copy does (0.15 ms idle, 7-10 ms under load).
- AMF signals the fence it is given again after reading a D3D12 texture. With
  one fence for all conversions, that signal released the next texture before
  its conversion ran: a third of the streamed pictures repeated. Each output
  texture now has its own fence.

Synthetic moving frames, AV1 HDR 1968×2184 paced at 120 fps
(`performance --synthetic 16 --paced`, `load_matrix.py`):

| Case | Graphics queue | Compute queues |
|---|---|---|
| Idle | 3.23 ms mean, 3.55 p95 | 2.92 ms mean, 3.04 p95 |
| Beside the load | 14.32 ms mean, 32.55 p95, 90 fps | 2.79 ms mean, 2.85 p95, 120 fps |

Full encrypted streams from the isolated host, AV1 HDR 1968×2184 at 120 fps
and 80 Mbps from a 240 Hz virtual display, decoded by an independent client
that times a moving barcode from render to decoded picture (`run-motion.py`
virtual-motion; same binary, `gpu_compute_conversion` off and on):

| Case | Picture age mean / p95 | New pictures/s | Host mean |
|---|---|---|---|
| Idle, graphics queue | 12.98 / 13.89 ms | 120.6 | 3.45 ms |
| Idle, compute queues | 12.37 / 13.29 ms | 120.1 | 2.86 ms |
| Load, graphics queue | 52.87 / 71.02 ms | 44.6 | 21.2 ms |
| Load, compute queues | 45.45 / 62.26 ms | 50.1 | 10.6 ms |

Under the load DWM's own composition dominates (its copy into the
duplication surface finishes 7-10 ms after the frame is handed over), so the
host's share falls but the picture stays late. Host time under compute
includes that wait, because frames are published before DWM's copy finishes
and the encoder waits for it on the GPU. These runs exclude network transport
and a remote display.

### 1080p at 60 fps

The same comparisons at the most common stream setting: HEVC 10-bit HDR,
1920×1080 at 60 fps and 20 Mbps, from a 120 Hz virtual display, with the
same game-like load (the game ran at 174 fps in both paths). Two runs each.

| Synthetic encode | Graphics queue | Compute queues |
|---|---|---|
| Idle, mean | 2.26 / 2.40 ms | 2.14 / 2.03 ms |
| Beside the load, mean | 21.99 / 20.93 ms | 1.98 / 1.95 ms |
| Beside the load, p95 | 37.04 / 37.91 ms | 2.28 / 2.29 ms |
| Beside the load, encoded fps | 58.6 / 59.1 | 60.1 / 60.1 |

| Full stream | Graphics queue | Compute queues |
|---|---|---|
| Idle, picture age mean / p95 | 14.05 / 15.03, 13.98 / 14.72 ms | 14.05 / 14.63, 14.04 / 14.69 ms |
| Load, picture age mean / p95 | 40.92 / 54.04, 41.13 / 54.69 ms | 33.48 / 42.20, 33.46 / 42.46 ms |
| Load, new pictures per second | 56.8, 56.6 | 58.3, 57.8 |
| Load, present to send (host counter) | 16.39, 16.10 ms | 11.40, 11.07 ms |

Idle, both paths deliver the picture at the same time; the compute path's
fence handoffs cost about 0.15 ms of host time at this size, where the
D3D11 copy and conversion are small. Beside the game it delivers the
picture 7.5 ms sooner on average and 12 ms sooner at the 95th percentile.
Artifacts: `day-work-20261004\load-1080p60`, `day-work-20261002\p1080-*`.

### Against Vibepollo 2.0

Vibepollo 2.0 and Rubylight 2.0.0-rc.2 alternated in one batch on the
same fixture. Vibepollo is a Release build of the 2.0.0 tag (`8a8c4b03a`)
with two startup-only patches for the isolated fixture (skip machine-wide
recovery, log the test display's name); no per-frame code changed. Both
hosts ran HEVC 10-bit HDR at 1080p60, 20 Mbps requested (14,988 kbps after
FEC and audio on both), native AMF at ultra-low latency and `speed` with
VBAQ and an input queue of 4, Desktop Duplication, realtime GPU priority,
and a 120 Hz virtual display (Vibepollo set to
`frame_limiter_auto_virtual_framegen = legacy`, its 2x mode; its default is
4x). Three runs each. The game-like load ran at 176.7-177.4 fps beside
Vibepollo and 173.8-174.9 fps beside Rubylight, which streamed about
twice as many frames. "Host latency" is the per-frame value Moonlight
reports from the host; both hosts measure it from the moment Desktop
Duplication hands over the frame to the moment the packet is sent.

| Case | Vibepollo 2.0 | Rubylight 2.0 |
|---|---|---|
| Idle, picture age mean / p95 | 16.03 / 16.62, 15.80 / 16.40, 16.05 / 16.93 ms | 13.88 / 14.58, 13.70 / 14.44, 13.73 / 14.52 ms |
| Idle, host latency | 2.73, 2.71, 2.80 ms | 1.96, 1.96, 1.97 ms |
| Load, picture age mean / p95 | 93.99 / 132.54, 92.04 / 129.45, 103.28 / 148.96 ms | 42.27 / 55.98, 42.31 / 56.81, 42.70 / 56.69 ms |
| Load, new pictures per second | 24.2, 25.0, 22.5 | 52.0, 51.0, 51.1 |
| Load, host latency | 59.09, 57.30, 65.28 ms | 9.11, 9.14, 8.80 ms |

Beside the load, Vibepollo handed AMF about 24 frames a second: its own
`encoder output has not caught up` lines count 60 submitted frames every
2.4-2.6 s. Rubylight's graphics-queue path delivered about 57 new pictures
a second beside the same load in an earlier batch, so sharing the graphics
queue alone does not explain the gap; where Vibepollo loses the frames has
not been traced. The probe window rendered 50-59 fps beside the load with
Vibepollo and 59 fps with Rubylight. Absolute load numbers move between
batches (Rubylight measured 33.5 ms in an earlier batch without Vibepollo
runs), so only compare rows measured together. Artifacts:
`day-work-20261002\hh1080-*`.

Both hosts' virtual-display policy also turns on an RTSS 60 fps limit
during a stream (Vibepollo logs it). A second batch, beside the load only,
ran each host with the limit and with `frame_limiter_provider = none`:

| Beside the load | Vibepollo 2.0 | Rubylight 2.0 |
|---|---|---|
| RTSS limit on, picture age mean / p95 | 124.84 / 156.05 ms | 42.68 / 57.60 ms |
| RTSS limit off, picture age mean / p95 | 125.81 / 158.30, 120.95 / 154.31 ms | 43.32 / 57.33 ms |
| New pictures per second, on / off | 18.6 / 18.5, 19.2 | 51.2 / 50.2 |
| Host latency, on / off | 79.7 / 79.8, 77.2 ms | 9.0 / 9.6 ms |

The limit changes neither host. One Rubylight run with the limit off is
left out: its probe window did not start. Vibepollo was slower in this
batch than in the first (about 124 against 96 ms) and the probe rendered
only 40 fps beside it, while Rubylight stayed at 42-43 ms. Artifacts:
`day-work-20261002\hh1080n-*`.

### HDR colour accuracy

`tests/colour_check.py` reads frame 900 of each stream above as decoded by
the client (`tests/moonlight_client.c` with `BUTTERPOLLO_TEST_FRAME_DUMP`),
recomputes the probe's scRGB picture for that frame, converts it as BT.2100
PQ with BT.2020 primaries in limited range, and compares. The
Vibepollo runs beside the load sent too few frames to reach frame 900, so
its column has the three idle runs; Rubylight's has all six.

| | Vibepollo 2.0 | Rubylight 2.0 |
|---|---|---|
| Black / 100-nit white patch (expected 64.0 / 509.08) | 64.0 / 509.0 | 64.0 / 509.0 |
| Luma error, mean absolute (10-bit codes) | 0.44 | 0.38-0.44 |
| Contrast slope (1 is exact) | 1.000 | 0.998-0.9995 |
| Saturation (decoded / expected chroma) | 97.9-98.0% | 100.2-100.6% |
| Chroma error, mean absolute | 0.96 | 0.30-0.59 |

Both streams also carry the same HDR10 metadata (BT.2020 primaries, D65, the
virtual display's peak luminance). Rubylight's decoded pictures match the
expected values within half a 10-bit code on average, with no lifted black and
no lost saturation in these reference frames. This validates the tested host
conversion and encoding path for this content and setup.

## Controlled comparison with the original C++ FEC

The benchmark loads the original C++ host's Reed–Solomon wrapper from baseline commit `f23ee0c9e7857887be7f774de6ac5153500a7e53`, with its pinned nanors implementation at `19f07b513e924e471cadd141943c1ec4adc8d0e0`. The source verification script checks every compiled reference file against its pinned SHA-256. GCC 16.1.0 builds the reference with `-O3 -ftree-vectorize -funroll-loops`; Rust uses the release profile. Both select AVX2 on this Ryzen 7 5800X3D. The original runtime ISA dispatch is retained.

| Data + parity shards, 1416 bytes each | Original C++ median | Rust median | Speedup | CPU time saved |
| --- | ---: | ---: | ---: | ---: |
| 32 + 7 | 6.96 µs | 5.51 µs | 1.26× | 21% |
| 96 + 20 | 63.03 µs | 45.87 µs | 1.37× | 27% |
| 192 + 39 | 250.01 µs | 178.72 µs | 1.40× | 29% |

Each result is the median of seven half-second runs, with Rust/C++ order alternated. Caller shard buffers and pointer views are allocated outside the timing; the original per-frame matrix allocation, encoding and release remain inside it. Every parity byte is compared before and after each round. A small generic Cauchy case (4 + 2 shards of 144 bytes) measures 0.139 µs in C++ and 0.133 µs in Rust, effectively equal; it is not the fixed Moonlight audio FEC matrix.

The Rust changes precompute finite-field inverses and reuse AVX2 input loads across four parity rows. SSSE3 and scalar fallbacks preserve the wire format. Tests cover every field coefficient, short and unaligned payloads, row-group tails and the 255-shard boundary. This is a component comparison against the previous implementation, with reproducible source inputs, rather than a comparison against an earlier slow Rust prototype.

```powershell
# Initialize third-party/nanors in a source checkout first.
pwsh -NoProfile -ExecutionPolicy Bypass -File .\rust\tests\build-fec-reference.ps1 -ArtifactDirectory C:\path\to\artifacts
cargo build -p butterpollo-core --example protocol_performance --release --locked
.\target\release\examples\protocol_performance.exe C:\path\to\artifacts\cpp-nanors-reference\nanors-reference.dll > fec.json
```

The release package includes the same probe as `butterpollo-protocol-performance.exe`. The reference DLL is a test fixture built separately; it is not shipped or linked into the Rust host. Building or running this comparison does not start either host or change display settings.

## Repeat-frame encoder capacity

The 2.0 candidate PyroWave record path completed **961 frames in 8.003 seconds at 3840×2160/120 HDR**, or **120.09 fps**, at an 800 Mbps target. Submission-to-observed-output time averaged **2.17 ms**, with **3.21 ms p95**. GPU conversion uses shared D3D11/Vulkan planar inputs and reads back only the encoded bitstream. This repeated-frame test covers conversion, encoding and framing; it excludes live capture, packetization/FEC, transport, decoding and display latency. It is a capacity measurement, not a whole-host comparison against C++.

```powershell
.\butterpollo-performance.exe --codec pyrowave --records --hdr --width 3840 --height 2160 --fps 120 --bitrate 800000 --seconds 8 --paced
```

The same real captured frame was repeatedly converted and encoded, with 20 warmup submissions followed by an eight-second unpaced run. Counts refer to returned encoded frames. Capture occurs before the timed loop; network, client decoding and display latency are excluded. Requested frame rate was 120; bitrates were 20/40/80 Mbps at 1080p/1440p/4K respectively.

| HEVC HDR output | Earlier Rust CPU path, fps | Rust GPU path, fps | GPU encode-call mean |
| --- | ---: | ---: | ---: |
| 1920×1080 | 6.41 | 689.08 | 1.45 ms |
| 2560×1440 | 3.77 | 420.75 | 2.38 ms |
| 3840×2160 | 1.69 | 202.68 | 4.93 ms |

The baseline is commit `319a3833b37119c6dac37339c7363bedc0bc0bee`, before this change. Both runs used the same capture dimensions, GPU and driver; the desktop content was not frozen across the runs. This is a repeated-frame capacity comparison against the earlier **Rust** implementation, not a controlled comparison against C++ and not a live-stream frame rate. Encode-call duration also excludes asynchronous codec completion; use completed-output latency or full-stream measurements to assess latency.

After setting AMF's AV1 alignment mode to allow unaligned input dimensions, the packaged probe produced 849.62 fps with 1080p HDR/120 input in a separate repeat-frame run. Previously initialization failed because AMF's default alignment excludes a 1080-line image. However, the full-stream decoder found a coded height of 1082, and the strict dimension check rejected that run. AMD documents two padded rows on this hardware for 1080p AV1; use HEVC for exact 1080p output or an aligned AV1 resolution such as 1440p/4K. This is not a validated exact-resolution 1080p AV1 result. [AMF's alignment documentation](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/wiki/AV1-Encoder#av1-specific-api)

That capacity run's submission-to-observed-output mean was 10.40 ms under an intentionally saturated queue; it is not a latency-oriented workload.

## Full encrypted Moonlight streams

The independent C fixture uses Moonlight-common-c for encrypted RTSP, control and UDP video/audio transport, then FFmpeg and Opus to decode the results. It rejects codec fallback and verifies the decoded dimensions, ten-bit HDR, BT.2020 primaries and PQ transfer. Each performance run lasts 20 seconds after connection initialization. Client frame rates include startup and buffered decoder frames; host steady rate uses session counter deltas after two seconds.

| Output and requested rate | Decoder threads | Host steady fps | Decoded fps | Completed encode mean / p95 |
| --- | ---: | ---: | ---: | ---: |
| H.264 SDR 1080p/120 | 4 | 120.01 | 118.11 | 2.29 / 2.60 ms |
| HEVC HDR 1080p/120 | 1 | 120.03 | 118.70 | 2.27 / 2.60 ms |
| HEVC HDR 4K/120 | 1 | 119.89 | 105.47 | 6.22 / 6.50 ms |
| HEVC HDR 4K/120 | 4 | 120.02 | 117.98 | 6.33 / 6.70 ms |
| AV1 HDR 4K/120 | 4 | 120.04 | 117.96 | 5.43 / 5.80 ms |

All five runs reported zero codec/Opus decoding errors. The single-threaded 4K decoder took 9.33 ms per submitted packet, exceeding the 8.33 ms frame budget; its receive queue eventually dropped packets and requested keyframes. The three latest four-thread runs enable `wgc_slot_aligned_publish=true` and `amd_ltr_frames=4`. HEVC 4K/120 received 2371 complete frames and decoded 2368 in 20.072 seconds; three frames remained buffered in the decoder. AV1 4K/120 received 2514 complete frames and decoded 2512 in 21.295 seconds. These client totals include startup/teardown, while host steady rates use counter deltas. Completed encode latency covers conversion/submission until the host observes encoded output, including asynchronous work. It excludes capture age, FEC/packetization, networking, decoding and display scanout.

### Final 2.0 candidate standard-codec checks

The final candidate repeated the three standard-codec checks with a 2560×1440
SDR capture source, a requested 120 fps and four software decoder threads. Each
encrypted stream ran for 20 seconds and passed strict dimensions/color,
permissions and client hooks with zero video/audio decode errors.

| Output | Host steady fps | Decoded fps | Host processing mean / p95 |
| --- | ---: | ---: | ---: |
| H.264 SDR 1080p | 120.03 | 118.13 | 3.37 / 6.70 ms |
| HEVC HDR 4K | 120.05 | 118.20 | 7.05 / 12.50 ms |
| AV1 HDR 4K | 120.04 | 118.26 | 7.14 / 13.00 ms |

The final wire-header processing metric includes capture age and completed encoding; PyroWave also includes its sender wait. It excludes packetization after the header, network transit, decoding and scanout. Earlier completed-encode measurements above exclude capture age, so their latency columns are not directly comparable. Client frame rates include startup/teardown.

## Vibepollo 2.0 PyroWave transport

The pinned Nonary Moonlight-common-c transport at
`d6a11bc685b41037b352a96f29d08276fe5359ba` receives/decrypts/FEC-recovers
record packets, then the independent vendor decoder renders to a CPU buffer.
Four 20-second encrypted streams at 1920×1080/120 and 200 Mbps pass with zero
video/audio decode failures or partial frames. The source is a 2560×1440 SDR
desktop.

| Profile | Host steady fps | Decoded fps | Host processing mean / p95 |
| --- | ---: | ---: | ---: |
| SDR 4:2:0 | 119.57 | 116.46 | 3.08 / 8.40 ms |
| HDR 4:2:0 | 119.95 | 116.51 | 3.08 / 9.60 ms |
| SDR 4:4:4 | 119.96 | 117.23 | 2.66 / 5.30 ms |
| HDR 4:4:4 | 119.97 | 117.44 | 1.88 / 2.40 ms |

Host processing includes capture age, completed encoding and waiting for the
PyroWave sender, as recorded in Moonlight's wire header. It excludes
packetization after the header, network transit, decoding and scanout. The
console's encode p95 measures the encoder separately. Client rates include
startup/teardown. The HDR 4:4:4 row is the final repeat after enforcing a
minimum one-tick advance on the 90 kHz RTP clock; it decoded all 2,357 received
frames with no partial frames.

At an 800 Mbps target, the same loopback sender is the limiting stage: a 30-second 1080p run delivers 101.49 steady host fps and decodes 2,993 frames (99.40 fps including startup). Host processing averages 8.10 ms with 14.30 ms p95. Older pending intra frames are replaced; they do not accumulate in an unbounded queue. This verifies continued decoding across many RTP sequence wraps and the visible backpressure counter, not 800 Mbps playback at 120 fps or a physical-network stress test.

The fixture requires MSYS2 UCRT64 GCC/CMake/Ninja, OpenSSL/Opus development libraries and the pinned patched PyroWave SDK. It is separate from the shipped Rust host:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File rust/tests/build-pyrowave-client.ps1 -ArtifactDirectory C:\tests\pyrowave -PyrowaveRoot C:\path\to\pyrowave-186f0393
$env:BUTTERPOLLO_TEST_CLIENT_EXE = 'C:\tests\pyrowave\moonlight-pyrowave-client.exe'
$env:PATH = 'C:\msys64\ucrt64\bin;' + $env:PATH
python rust/tests/interop.py C:\tests\pyrowave pyrowave-hdr-444 1920 1080 120 20 200000 4
```

## October 7: PyroWave recovery feedback

The rc.19 report describes about 3.3 `idr_requests` per second at several
PyroWave bitrates. That counter alone cannot identify the incoming message:
it included explicit IDR requests, reference invalidations that fell back to
IDR, and host requests such as encoder startup. The exact reporting client
build and its control trace were not available for this investigation.

The pinned [Nonary control
implementation](https://github.com/Nonary/moonlight-common-c/blob/d6a11bc685b41037b352a96f29d08276fe5359ba/src/ControlStream.c)
uses `0x0301` for reference invalidation and `0x0302` for an explicit IDR
request. Its older `0x0201` loss report runs every 50 ms; modern Sunshine
connections instead send `0x0200` pings every 100 ms and queued `0x5502` FEC
status reports. Rubylight ignores those statistics messages for recovery. This
client's decoder-capability check enables RFI only for H.264, HEVC and AV1;
PyroWave transport loss requests an IDR instead. Neither this client transport
nor the host has a 300 ms recovery timer. The [Qt PyroWave
decoder](https://github.com/Nonary/moonlight-qt/blob/43225b52c934174736123f894580c0decbe4bec2/app/streaming/video/ffmpeg.cpp)
also returns `DR_OK` after rejecting a PyroWave picture: the next independent
picture replaces it.

There was a host-side feedback bug. PyroWave reference invalidation reached
an unsupported encoder method, requested an IDR and increased `idr_requests`.
Both pending invalidation and the IDR latch could bypass the capture cadence,
including encoding a repeated picture early. The PyroWave encoder itself
ignores the IDR argument, so it never produces a larger recovery keyframe.
Critical FEC uses the record layout and configured percentage; detail FEC uses
frame cadence, record stability and the available wire budget. Neither reads
these control messages. Extra early pictures could indirectly change cadence
and bandwidth, but feedback does not directly increase parity.

PyroWave feedback now leaves the cadence latches alone. Reference invalidations
have their own `reference_invalidations` counter in session statistics and
the periodic timing log, and cannot become PyroWave IDR requests. Explicit
`0x0302` requests remain visible in `idr_requests`, including for PyroWave;
the existing host startup/fallback accounting is retained. This follows
[Vibepollo 2.0's handlers](https://github.com/Nonary/vibepollo/blob/8a8c4b03a/src/stream.cpp),
which separately count IDR/RFI messages and ignore both for PyroWave encoding.
Unit tests cover valid and malformed PyroWave invalidations, explicit requests,
and unchanged H.264/HEVC/AV1 range merging and IDR fallback.

The framing comparison found the same eight-byte short header, IDR frame type
2, exact final payload length, critical-prefix packet count, `0x80` record-start
flag, RTP/24-bit stream sequence progression and up-to-four-block FEC layout.
The first FEC block protects the critical records at 20% by default, with at
least two parity packets, when it fits the 255-shard Reed-Solomon limit;
otherwise the planner uses unprotected blocks. The critical-packet count is not a required ratio of
the whole frame; low-bitrate pictures can devote a larger share to coarse
records. No framing or FEC changes were justified by the comparison.

Before changing the host, the independent pinned receiver streamed record-framed
PyroWave over loopback at 2560×720/60, with 10/200/800 Mbps requested and
9,308/198,988/798,988 kbps negotiated. It decoded all 682/683/715 received
pictures, with zero partial frames and zero decode failures. Each run kept
`idr_requests=1` from startup through teardown; the 3.3-per-second symptom did
not reproduce. These are transport/vendor-decoder checks, not validation of
the reporting client's renderer. The release e2e wrapper correctly reports
missing motion/audio-continuity measurements for this PyroWave receiver; its
interoperability check passes, but it is not a full release-e2e pass.

### Radeon LTR recommendation (evaluation only)

Keep `amd_ltr_frames=0` as the default for now. AMF supports explicit LTR
selection for [HEVC](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/doc/AMF_Video_Encode_HEVC_API.md#228-ltr-properties)
and [AV1](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/doc/AMF_Video_Encode_AV1_API.md#227-ltr-properties).
Rubylight can already recover a valid reference-invalidation range from an
older retained anchor, reducing the need for an IDR. The existing strict
FFmpeg omission fixtures above decoded all 56 retained pictures and exercised
two LTR recoveries each for HEVC and AV1. That is correctness evidence, not a
measured Wi-Fi or RDNA4 improvement.

LTR cannot replace explicit decoder-reset IDRs, a lost initial anchor, invalid
ranges or recovery after all usable anchors are gone. It also needs a client
decoder that supports reference invalidation and enough negotiated references
for the anchors plus the rolling reference. The current policy respects that
budget, disables LTR with intra refresh, checks driver capabilities/readback,
and falls back on rejected surface properties.

For a supported Radeon/client pair, opt-in LTR is worth testing with controlled
loss, delayed feedback and decoder resets while measuring recovery bytes,
latency and sustained decoding. Test RDNA4 separately: the documented
[RX 9000 H.264/HEVC freeze report](https://github.com/AlkaidLab/foundation-sunshine/issues/666)
Do not enable LTR globally to mask congestion. No encoder policy or AMF code
was changed by this investigation.

## October 7: Wi-Fi and unknown-route pacing

H.264, HEVC and AV1 default to twice the negotiated encoder bitrate only on
confirmed Wi-Fi (interface type 71) or mobile broadband (237, 243, 244) host
routes, bounded to 1–800 Mbps. Ethernet, loopback, VPN, Tailscale and unknown
routes retain the rc.19 default of 800 Mbps. A known physical Ethernet link
caps that at 80% of its reported speed. Virtual Ethernet adapters, including
Hyper-V vSwitches, use the physical adapter's type and speed when Windows'
interface stack exposes an unambiguous binding. Missing or ambiguous bindings
keep the wired default. This corrects the initial policy that treated every
route without a hardware Ethernet speed as wireless. The first frame resolves
the route before selecting its rate; subsequent lookups retain the two-second
refresh.

Positive overrides retain their existing 110% stream-bitrate floor and 80%
physical-link cap. PyroWave's automatic sender is unchanged: 95% of a known
Ethernet link, or per-frame wire demand and its bitrate floor otherwise.
Its independent-frame bandwidth must not inherit an 800 Mbps ceiling or a
conventional-codec multiplier. The rc.19 two-frame encoder gate is unchanged.

**Wire-time estimate, not a measurement:** a 1,000,000-byte keyframe takes
80 ms at 2×50 Mbps versus 10 ms at 800 Mbps, before packet overhead and send
cost. Classification tests cover wireless, mobile, virtual, unknown and
loopback routes plus resolved, missing and ambiguous physical bindings.
No live stream or physical-route performance test was run for this correction.

### Measured loopback cost

These runs used the release `rust/release/e2e.py` fixture, WGC and an RX 7900 XT
with driver `32.0.31041.1004`. The installed service log was checked before
each run and showed no active stream. The portable host used its own package,
configuration and ports. Other agents could still use the GPU, so settings
and builds were alternated, with two runs per case; ranges below are the two
observations, not confidence intervals. Baseline was commit `4ea3b058`.
Raw logs, receiver results and sampled statistics are in this worktree's
`target/network/measure`; `target/network/measurements.json` contains the
summary. No physical Wi-Fi path was measured.

The first six runs compared pacing settings on the baseline executable at
2560×720/60, with 60,000 kbps requested and 46,988 kbps at the encoder.
Overrides of 93,976 and 70,482 kbps modelled 2× and 1.5× on loopback.
The moving-strip scene actually sent only 9.8–11.2 Mbps of UDP video, including
FEC; these timings are not a sustained 60 Mbps traffic test. Send cost below
is the mean of sampled `present_to_send_mean_ms - frame_age_mean_ms -
host_processing_mean_ms` after three seconds, including packetization and
send waits. It is not encode time or client latency.

| Baseline pacing | Send cost per frame | Receiver steady FPS | Release e2e results |
| --- | ---: | ---: | --- |
| 800 Mbps | 0.16–0.17 ms | 60.57–60.70 | Both passed |
| 2× encoder bitrate | 0.65–0.81 ms | 60.58–60.77 | One passed; one audio-continuity failure |
| 1.5× encoder bitrate | 1.84–1.90 ms | 60.57–60.60 | One passed; one audio-continuity failure |

All six decoded every delivered picture without a decode failure. The lower
send cost and larger FEC/scheduling margin favour 2× over 1.5×.

A separate alternating baseline batch at 5120×1440/240, 150,000 kbps requested
and 118,988 kbps encoded, exposed the cost of applying 2× unnecessarily to a
fast route. Host throughput fell from 215.61–216.01 FPS at 800 Mbps to
206.43–207.63 FPS at 237,976 kbps, about 4%. This is why loopback retains its
old default. The software receiver could not sustain the requested rate, so
these runs failed the receiver cadence/motion gates; they are host throughput
measurements, not successful 240 FPS playback.

The final baseline/candidate/baseline/candidate batches gave:

| Workload | Baseline host FPS | Candidate host FPS | Baseline / candidate send cost |
| --- | ---: | ---: | --- |
| HEVC 2560×720/60, 800 Mbps versus 2× override | 60.54–60.60 | 60.59–60.61 | 0.12–0.14 / 0.52–0.61 ms |
| HEVC 5120×1440/240, automatic loopback | 216.82–218.14 | 217.73–218.05 | 0.32–0.43 / 0.32–0.33 ms |
| PyroWave 2560×720/120, 800 Mbps requested, automatic | 87.97–88.02 | 88.10–91.50 | 11.26–11.36 / 10.87–11.26 ms |

All four normal HEVC runs passed the complete release fixture, decoded all
927–938 pictures, and retained continuous audio. Receiver steady FPS was
60.58–60.61; arrival-interval p99 was 18.47–22.95 ms. Candidate mean
presentation-to-send time was 3.43–3.60 ms versus baseline 3.10–3.73 ms.
The saturated HEVC runs again failed receiver cadence/motion gates: the
software receiver achieved only 36.05–55.97 FPS and requested recovery frames.
All had continuous audio and zero decode failures among delivered pictures.
There was no host throughput regression in these loopback observations.

The PyroWave receiver decoded all 4,369 pictures across the four runs, with
zero partial frames, zero decode failures and `idr_requests=1` throughout;
candidate `reference_invalidations` stayed zero. Actual UDP video was 581–611
Mbps on the candidate versus 586–592 Mbps on baseline. These runs passed the
independent receiver's interoperability check, but the release wrapper reports
its missing motion/audio metrics, as described above.

### Estimated burst size

For a hypothetical 60 Mbps encoded stream at 60 FPS, each average picture is
125,000 bytes. With a 1,392-byte packet setting, encryption, IPv4 and 20% FEC,
that is 91 data packets plus 19 parity packets: 165,660 modelled wire bytes.
Serialization takes about 11.04 ms at 120 Mbps (2×), 14.73 ms at 90 Mbps
(1.5×), or 1.66 ms at 800 Mbps, within a 16.67 ms frame period. This explains
why 1.5× leaves little allowance for larger pictures or scheduling delays.

With the default 64 KiB batch limit, the sender's two-millisecond budget
reduces an initial encrypted UDP burst from 45 packets / 64,800 bytes at 800
Mbps to 20 packets / 28,800 bytes at 120 Mbps.

## CPU fallback conversion

This comparison used exactly the same synthetic 1968×2184 FP16 scRGB image, with patterned RGB data, resized to each output dimension. Each version ran for at least three seconds. The new CPU implementation uses lookup tables, precomputed resize columns and at most eight Rayon workers, with a serial fallback if worker creation fails.

| Output | Earlier Rust mean | New Rust mean | Speedup | Maximum 16-bit output difference |
| --- | ---: | ---: | ---: | ---: |
| 1920×1080 | 134.41 ms | 8.89 ms | 15.1× | 0 |
| 2560×1440 | 240.34 ms | 14.67 ms | 16.4× | 0 |
| 3840×2160 | 539.96 ms | 34.26 ms | 15.8× | 0 |

These timings cover RGB HDR conversion/scaling only, not GPU upload or
encoding. Unit tests separately check FP16 decoding, PQ reference luminances,
linear-light resizing and already-PQ ten-bit input.

## Color and ownership checks

The Rust GPU converter keeps absolute luminance through 10,000 nits. A separate HEVC decode of grayscale patches at 0/80/1000/10000 nits returned limited-range P010 luma codes 64/490/723/940 and neutral chroma 512/512. Hardware tests compare primaries and grayscale against a CPU reference within two ten-bit codes, check a linear-light resize and verify that frames retained by the codec are not overwritten when the bounded pool is reused. New 4:4:4 tests verify independent adjacent chroma and planar ten-bit HDR codes. The packed AYUV shader is checked through its compatible RGBA render-target view on AMD; actual AYUV resource allocation and NVIDIA CUDA interop require NVIDIA hardware. The abandoned vendor conversion path clipped the 10,000-nit patch; that path is not used.

Native Winsock checks delivered eight separate datagrams in two send calls on
both IPv4 and IPv6, then verified the ordinary-send fallback and a short
trailing datagram. Video batches respect the previous 16/32/64 KiB setting, a
64-packet/65,507-byte ceiling and a two-millisecond wire budget.

Independent strict FFmpeg loss fixtures encode 64 frames, omit frames 5–8 and 17–20, and decode all 56 retained frames for H.264, HEVC and AV1. HEVC/AV1 use two LTR recoveries; H.264 uses one LTR recovery and an IDR fallback at its reference-counter wrap. Actual Opus round trips cover 21 stereo/5.1/7.1/custom-layout, quality and packet-duration combinations, preserve every channel and stay inside the transport packet budget. These checks establish recovery/audio correctness, not an end-to-end latency improvement.

## Reproduce on another machine

The release ZIP includes `butterpollo-performance.exe`, which captures one frame and prints JSON. It does not start a service or change display configuration. A functioning local display/capture session and the relevant codec/GPU driver are required.

```powershell
.\butterpollo-performance.exe --encoder amf --codec hevc --hdr --width 3840 --height 2160 --fps 120 --bitrate 80000 --seconds 8
.\butterpollo-performance.exe --encoder amf --codec hevc --hdr --width 3840 --height 2160 --fps 120 --bitrate 80000 --seconds 8 --paced
.\butterpollo-performance.exe --encoder amf --codec hevc --hdr --width 3840 --height 2160 --fps 120 --bitrate 80000 --seconds 8 --cpu
```

Unpaced mode measures capacity with a bounded queue; paced mode measures completed-output latency at the requested cadence. `--cpu` reads back the captured frame once and measures the CPU compatibility encoding path. JSON reports source dimensions, adapter, returned-frame count, call time and submission-to-observed-output time. The source probe is `windows/examples/performance.rs`; `cargo build -p butterpollo-windows --example performance --release --locked` builds it with the SDK environment from the main build.

For full streams, use an isolated host configured with base port 48123, the fixture credentials `test` / `rust-smoke-only` and a Desktop app. These are test-only credentials for a loopback fixture. `tests/build-moonlight-client.ps1` builds the independent C client against the initialized Moonlight-common-c submodule. The MSYS2 UCRT64 environment additionally needs FFmpeg decoding development libraries and OpenSSL (`mingw-w64-ucrt-x86_64-ffmpeg` and `mingw-w64-ucrt-x86_64-openssl`), alongside Opus, CMake, Ninja and GCC. Python needs `requests` and `cryptography`. The probe creates a client certificate, performs real PIN pairing, grants only its fixture launch permissions and cancels/unpairs that fixture afterward.

```powershell
$env:BUTTERPOLLO_TEST_PORT = '48123'
.\rust\tests\build-moonlight-client.ps1 -ArtifactDirectory C:\path\to\artifacts
python rust/tests/interop.py C:\path\to\artifacts hevc-hdr 1920 1080 120 20 20000 1
python rust/tests/interop.py C:\path\to\artifacts hevc-hdr 3840 2160 120 20 80000 4
python rust/tests/interop.py C:\path\to\artifacts av1-hdr 3840 2160 120 20 80000 4
```

The positional arguments after the artifact directory are codec, width, height, requested fps, duration, requested bitrate in kbps and software decoder thread count. Reports contain per-half-second host counters and the derived host steady rate. Requested Moonlight bitrates can be adjusted during negotiation; the actual encoder bitrate is in each session sample.

## October 5: WGC startup and notification experiment

The OpenCode investigation ended with a strict WGC smoke test succeeding as the
interactive user but failing as SYSTEM in that user's session:
`CreateForMonitor` returned `0x80070424`. The Rust service runs its host under
that SYSTEM token. Capture recovery already tried Desktop Duplication when WGC
could not open; initial stream startup did not. Startup and recovery now share
the fallback, log the actual backend and retain both Windows errors if neither
backend opens. Strict probes still fail rather than substitute DDX.

This is a fallback, not service-mode WGC support. A capture helper running as
the signed-in user is still needed. DDX fallback is not evidence of equivalent
VRR or game-frame-generation behavior.

The next experiment used WGC's
[frame-arrival callback on the pool's worker thread](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.createfreethreaded)
to wake capture, replacing 500 us polling with notification waits bounded by a
100 ms housekeeping timer. Eight ten-second runs used the same release probe,
in poll/notify/notify/poll order for each GPU condition. The source was the
existing physical desktop, with approximately 35 changing pictures per second;
the first second was excluded from latency samples. GPU load was the existing
offscreen `gpu_load 55 1000 0 200` fixture. The installed host remained idle.

| Condition | Wake method | Frames sampled | Mean detection | Per-run p95 | Empty pool checks per ten seconds |
| --- | --- | ---: | ---: | ---: | ---: |
| Idle | 500 us polling | 632 | 0.566 ms | 1.247 / 1.487 ms | 18,884 / 18,898 |
| Idle | Notifications | 641 | 0.391 ms | 0.889 / 1.006 ms | 360 / 346 |
| GPU load | 500 us polling | 645 | 4.527 ms | 9.647 / 9.655 ms | 18,938 / 18,941 |
| GPU load | Notifications | 657 | 5.133 ms | 11.678 / 12.070 ms | 393 / 349 |

Detection means WGC's `SystemRelativeTime` to the host's snapshot acquisition,
including WGC's own delivery delay. The means above are weighted by frame
count; p95 values belong to individual runs. These are capture-component
measurements on changing desktop content, not a controlled game or an
end-to-end WGC/DDX comparison.

Notifications reduced idle detection by 0.175 ms and almost eliminated empty
polls, but increased loaded detection by 0.606 ms in this batch. Therefore
**production capture keeps polling**. Callback registration and waits are
opt-in for `windows/examples/wgc_arrival_probe.rs`, not enabled by normal
streams. An exploratory hybrid run had zero updated frames after warmup and
overlapped part of a build; it is excluded and establishes no latency result.

The callback experiment also exposed a Windows teardown trap: revoking
`FrameArrived` after closing the pool aborts in `GraphicsCapture.dll` instead
of returning an error. The probe's registration is revoked before closure and
only once; the event remains owned until in-flight callbacks return. Sixteen
native reconnect/COM-teardown cycles pass with this order. Workspace tests
and warnings-as-errors checks pass too.

The final release build also passed an isolated encrypted user-mode WGC stream:
1920x1080 HEVC SDR at 60 FPS, 20 Mbps requested, 12 seconds. The independent
client decoded all 715 received frames with zero failures and decoded 2,222
audio packets with a nonzero test tone. The host log explicitly reports
`requested=wgc backend="wgc"`; capture was the existing 5120x1440 SDR physical
desktop, with display changes disabled. This establishes functional
capture/encode/transport/audio interoperability, not a motion latency
improvement or service-mode WGC acceptance.

Local raw results, guards and validation logs are in
`%USERPROFILE%\.codex\artifacts\butterpollo-wgc-20261005`.
The stream's full logs are in the earlier fixture's
`day-work-20261002/codex-20261005-wgc-startup` directory. The final host SHA-256
is `c10eeb65724d0598b7869dab6f6a6ace7715d2b46da06e0ab85640339eb3e0c2`.
The measured prototype's SHA-256 is
`b2b7e36667f7317864c0f8a31a01f2f5d4077691d02a69de398988a2121e68e9`.
For further investigation on a changing desktop, build the release
`wgc_arrival_probe` example and run `wgc_arrival_probe DISPLAY 10 poll`,
`wgc_arrival_probe DISPLAY 10 notify`, or `wgc_arrival_probe DISPLAY 10 hybrid`.
An empty display argument selects the primary output. A report with zero
post-warmup samples has no valid detection-latency comparison.

## October 5: LAN pacing and encoder follow-up

The reporter identifies an RX 9070 XT, the latest driver and Wi-Fi. Their rc.2
log contains one HEVC hardware instance, repeated DDX access-loss recovery, and
transient UDP errors 10055 and 10035. This workstation has an RX 7900 XT with
two reported HEVC instances; disabling multi-instance encoding does not
reproduce the reporter's GPU.

The independent receiver at `192.168.4.10` is an i5-8259U / Iris Plus 655 NUC
running Debian 13 on gigabit Ethernet. The Windows sender uses 2.5 Gb Ethernet.
The receiver uses Moonlight-common-c
`2600beaf13f18bfa43453609cf5e3b84a4227760`, FFmpeg and Opus in an isolated
Docker image. No host packages, network settings or installed service profile
were changed. Streaming profiles disable display mode/HDR changes; the later
motion-fixture refresh correction is documented below. A guard stops test-owned processes if an installed
stream or application becomes active.

### Packet pacing

Video pacing now measures its next wait from completion of the preceding
batch. A delayed socket call or wakeup can no longer accumulate credit for a
catch-up burst. Pacing includes Ethernet/IP/UDP wire overhead. Known physical
Ethernet routes cap the burst rate at 80% of link speed; unknown routes retain
the existing 800 Mbps default. This measures the sender's local Ethernet link,
not end-to-end capacity. It does not infer a Wi-Fi client's capacity through a
wired access point. Explicit pacing limits remain useful for that case, and
the stream bitrate and picture settings are unchanged.

The UDP probe sends 46,042 deterministic 1,400-byte datagrams: 600 frames at
60 FPS, 50 Mbps payload, with two keyframes eight times larger. The independent
Linux receiver records kernel timestamps, integrity and socket-overflow counts.
Its effective receive buffer is 425,984 bytes; Python processing and this buffer
are part of the stress fixture, not Moonlight's normal receiver.

| Pacing case | Actual receiver overflows | Complete frames | Mean frame send span |
| --- | ---: | ---: | ---: |
| Previous 800 Mbps, two runs | 344 / 400 | 598 / 599 | 0.693 / 0.693 ms |
| Revised 800 Mbps | 129 | 599 | 0.779 ms |
| Revised 80 Mbps | 0 | 600 | 10.728 ms |

The slower cap delivered every datagram without corruption, at the cost of a
longer send span. In a separate **modeled** 100 Mbps / 128 KiB bottleneck,
adding a 6 ms sender stall raised peak queued bytes to 93,357 with previous 80
Mbps pacing, versus 39,694 with completion-based pacing. Both corresponding
real receiver runs delivered every datagram. No sender-side 10055/10035 error
was reproduced. Raw results: `udp-results.json`; probe:
`windows/examples/udp_pacing_probe.rs`.

### Encoder comparisons and rejected output-wait change

The controlled encoder fixture uses eight deterministic moving input textures,
HEVC 3840x2160 at 60 FPS / 50 Mbps, ULL/speed, VBAQ enabled and preanalysis
disabled. Each run encodes 480 pictures; comparisons use two runs per setting
in reverse order, separately idle and under the same offscreen GPU load.

| Existing conversion path | Idle mean | Loaded mean | Loaded per-run p95 |
| --- | ---: | ---: | ---: |
| Compute, current default | 6.339 ms | 5.955 ms | 6.209 / 6.203 ms |
| Graphics | 6.223 ms | 8.512 ms | 10.948 / 10.978 ms |
| Compute, multi-instance disabled | 6.320 ms | 5.948 ms | 6.178 / 6.183 ms |

This confirms the benefit of Opus's existing compute path under load on this
GPU; it is not a newly implemented speedup or a whole-stream comparison.

Removing an extra timer wait after AMF's own blocking output query reduced
loaded component mean latency from 5.958 to 5.831 ms. It also reduced idle
1080p60 LAN stream host mean from 2.279 to 2.201 ms and p95 from 2.6 to 2.3 ms.
However, the loaded LAN comparison regressed from 6.102 to 6.501 ms mean, with
per-run late-interval counts 89/76 before versus 83/97 after. All eight runs
decoded successfully and sustained approximately 60 FPS. The source was the
existing desktop scaled from 5120x1440, not controlled full-screen motion.
**The production output-wait change was reverted.** Component gains alone did
not justify the loaded stream regression. Raw results are in
`encoder-controlled`, `encoder-polling` and `lan-poll2-results.json`.

### Independent receiver limits and reproduction

A native DDX snapshot test failed twice with no initial image. The physical
display's existing idle timeout is 180 seconds. Holding a temporary display
power request made the same unchanged test pass once in 0.36 seconds, but later
repeats failed even with that request and a moving test window. Unlike the C++
host's capture loop, the Rust capture worker had no request to prevent display
sleep, which is a separate missing behavior. Capture now holds
`ES_DISPLAY_REQUIRED | ES_CONTINUOUS` for its lifetime, preserving prior thread
requirements and restoring them at teardown. The guard cannot move between
threads. The snapshot fixture uses the same guard.
[Windows documents the request and restoration semantics here](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadexecutionstate).
This is a capture-liveness correction, not evidence that display sleep caused
the reporter's access-loss events or remaining encoder-latency difference.

Two 230-second 1080p60 HEVC LAN runs crossed the existing 180-second display
timeout. Input activity was recorded; the last input occurred at connection
startup, with more than 230 seconds idle by each run's end. In the control,
all ten five-second samples after 180 seconds had no fresh capture claims;
the revised worker continued recording fresh claims in all ten. Sending
repeated pictures can hide this problem behind an apparently healthy FPS.

| Display request | Received / decoded pictures | Steady FPS | Host mean / p95 | Late arrival intervals |
| --- | ---: | ---: | ---: | ---: |
| Previous behavior | 13,743 / 13,743 | 60.139 | 2.290 / 2.6 ms | 51 |
| Held during capture | 13,838 / 13,838 | 60.561 | 2.270 / 2.6 ms | 44 |

Both runs decoded nonzero audio with zero codec failures. Both still needed
two DDX restarts during startup and initially received some blank pictures;
the awake request does not resolve that startup issue. This single long pair
validates continued fresh claims in this idle environment, not a general
latency improvement. Reports: `lan-power-results.json` and each run's
`capture-freshness.json`. The native power-state test separately checks prior
requirements, nested guards and restoration.

Every hardware-decoded picture is read back before checking exact geometry,
bit depth, HDR signalling and pixel contrast. Audio validation requires a
decoded test tone. A 1080p60 HEVC run decoded 1,203/1,203 pictures, sustained
60.597 FPS after warmup and had zero intervals above 1.5 frame periods.
The 4K60 HEVC run decoded 538/538 received pictures without errors, but the
NUC's decoder plus readback averaged 35.023 ms and delivered only 27.588 FPS.
That run is **not a performance pass**, despite correct pictures and audio.
The receiver now supports `BUTTERPOLLO_TEST_MIN_FPS` to fail such runs directly.
The initial separately named comparison binary was unreachable from the NUC;
it produced no stream and is excluded. Subsequent comparisons used the same
test executable path and saved each binary's hash.

Final 18-second runs of the retained changes passed the minimum-rate gate,
exact dimensions, visible pixel contrast on every decoded picture, and the
test tone. The source remained the existing SDR desktop; the HDR row checks
SDR-to-HDR conversion and HDR signalling, not native HDR capture.

| Backend / codec | Stream size | Received and decoded | Steady FPS | Decode failures |
| --- | --- | ---: | ---: | ---: |
| DDX / H.264 | 1920x1080 | 1,082 | 60.591 | 0 |
| DDX / HEVC Main10 HDR | 1280x720 | 1,082 | 60.582 | 0 |
| DDX / AV1 | 1280x720 | 1,083 | 60.614 | 0 |
| WGC / HEVC | 1920x1080 | 1,088 | 60.601 | 0 |

AV1 used software decoding; the other rows used Intel VAAPI plus readback. The
WGC row checks the actual opened backend, so fallback cannot pass it as a WGC
result. Reports are in `lan-final-results.json`. That earlier workspace run
passed 198 tests, including 18 native checks, with the desktop active. The
earlier inactive-desktop DDX failures remain recorded; a later pass does not
close that condition. The excluded AV1 geometry check independently fails all
twelve requested SDR/HDR/alignment combinations.

The Windows independent receiver also passed 1280x720 HEVC (718/718 pictures,
60.649 steady FPS), and correctly rejected an intentionally impossible
1,000 FPS requirement while still decoding 718 pictures without codec errors.
The final rebuilt host passed a local 3840x2160 HEVC / 60 FPS / 50 Mbps stream:
985/985 received pictures decoded, 60.585 steady FPS, 6.269 ms mean / 6.6 ms
p95 host time, and nonzero audio. The whole-run rate was only 54.56 FPS because
startup recovery consumed part of the 18-second run; 200 initial pictures had
no sampled luma contrast. Keep that startup defect visible. This source was
the existing 5120x1440 SDR desktop scaled to 4K, not native 4K motion capture.
Reports: `local-final-hevc`, `local-rate-gate-negative`, `local-final-4k-hevc`.

Build `tests/build-moonlight-client.sh ARTIFACT_DIRECTORY MOONLIGHT_SOURCE`
on Linux with CMake, C/C++ compilers and OpenSSL, FFmpeg and Opus development
packages. Use `BUTTERPOLLO_TEST_HOST` and `BUTTERPOLLO_TEST_PORT` with
`tests/interop.py` against a test-owned host profile. Optional variables are
`BUTTERPOLLO_TEST_HW_DECODER=vaapi`,
`BUTTERPOLLO_TEST_HW_DEVICE=/dev/dri/renderD128`,
`BUTTERPOLLO_TEST_REQUIRE_PICTURE=1`, `BUTTERPOLLO_TEST_AUDIO_TONE=1`,
`BUTTERPOLLO_TEST_WARMUP_SECONDS=5` and `BUTTERPOLLO_TEST_MIN_FPS=58.2`
for a 60 FPS run. The profile must repeat static pictures at the requested
rate, or a static desktop's deliberately reduced rate will fail this gate.
Remote clocks are independent, so remote measurements do not subtract the
sender's QPC timestamps or claim source-to-display latency.

The remaining RX 9070 XT acceptance comparison needs that GPU and client:
keep the same 3840x2160 / 60 FPS HEVC profile, bitrate, range and game scene
on both hosts; record the actual AMD driver version, not just "latest".
Compare idle and loaded runs in alternating order after warmup. On Rust,
compare compute conversion enabled and disabled without changing quality.
Repeat the same client once on Ethernet, then on Wi-Fi, to separate encoding
from delivery. Preserve capture-restart and UDP-drop logs alongside host
processing, frame age and delivery intervals. No setting from a faster local
GPU or a wired fixture closes that acceptance check by itself.

## October 5 WGC compute copy and capture startup follow-up

WGC can use the same fenced D3D12 copy and AMF conversion as DDX on supported
AMD devices with `wgc_compute_copy=true`. It remains opt-in after the production
repeat-rate comparison below found a loaded cadence tradeoff.
`gpu_compute_conversion=false` disables compute globally. WGC's graphics-copy
and polling defaults remain unchanged. Failure to initialize
compute preserves WGC on D3D11, and a texture-sharing failure now copies the
**same frame** on D3D11 instead of waiting for another desktop update.
A native regression test failed before that fallback fix and passes after it.

Before measuring latency, 120 WGC frames at idle and 120 under GPU load were
compared byte for byte with D3D11 readback of the same source frame. Every
comparison passed; each run contained 119 actual content changes. The
candidate was read first so reading the reference could not hide a missing
synchronization fence. A retained snapshot also remained unchanged after
capture teardown. These were SDR desktop captures on the local RX 7900 XT.

Eight initial 22-second HEVC streams used graphics/compute/compute/graphics
order both at idle and beside `gpu_load 45 1000 0 200`. The motion fixture
temporarily requested 60 Hz on the 5120×1440 SDR display, with a deterministic
128-pixel moving strip; the stream was 2560×720 at 60 FPS and 20 Mbps. These
initial runs forced `minimum_fps_target=60`. Scaling by exactly one
half preserved the timestamp barcode. The first five seconds were excluded.
All received pictures decoded correctly, with 100% barcode coverage, nonzero
test-tone audio, and no capture restart. Picture age uses the same PC's QPC
clock and includes decoding; it excludes scanout and input latency.

| GPU condition | WGC copy/conversion | Host mean | Decoded picture-age mean | Distinct pictures/second |
| --- | --- | ---: | ---: | ---: |
| Idle | Graphics | 2.361 ms | 31.432 ms | 46.758 |
| Idle | Compute | 2.210 ms | 31.835 ms | 50.601 |
| Loaded | Graphics | 8.557 ms | 49.052 ms | 49.704 |
| Loaded | Compute | 1.953 ms | 40.630 ms | 50.357 |

Values are the mean of two runs per cell. Under load the host mean fell by
77% and decoded picture age by 17%; both reversed-order runs agreed. Idle
picture age did not improve. Transport FPS was 60.425–60.645, which must not
be confused with distinct-picture FPS. The producer itself slowed to about
56.7 FPS with the graphics path and 58.8 FPS with compute under this load.
This is a controlled strip plus offscreen load, not a full game or native 4K.

An initial two-run pilot found fewer distinct frames with compute (50.386
versus 45.726 FPS). The full alternating comparison above did not reproduce
that ordering. The deliberately strict 50-distinct-FPS gate still failed on
both idle graphics runs and one idle compute run, and on both loaded graphics
runs. Artifacts: `wgc-compute-verification`, `wgc-compute-abba`, and
`wgc-compute-abba2` under the October 5 artifact directory.

### Fixture correction and production repeat rate

The original motion probe changed physical refresh when passed a numeric
rate. It now paces animation with a timer and never changes display mode.
Read-only checks before, during and after its 60-FPS animation confirmed
5120×1440 at the saved 240 Hz; the three-second probe produced 60.322 FPS.
The longer fixture produced 7,501 pictures over 125.000 seconds. The Windows
QPC frequency was independently confirmed as 10 MHz. No restoration was
needed: current and saved display modes already agreed at the audit.

Four 18-second WGC compute runs, in 60/20/20/60 repeat-rate order, isolated
the difference between the fixture's forced repeats and production's existing
`minimum_fps_target=20`. With 60-FPS motion on the unchanged 240-Hz display,
forced repeats delivered 44.085/45.608 distinct FPS and 21.333/21.094 ms mean
picture age. Production's repeat rate delivered 59.925/60.002 distinct FPS
and 11.997/11.805 ms. All pictures decoded, barcode coverage was 100%, and
there were no capture restarts or late intervals. Thus the earlier repeated
pictures are partly a fixture artifact. The production repeat default was
already correct and remains unchanged. Report: `repeat-cadence`.

Final workspace/native validation passes 200 checks, including 20 native
checks, with the corrected moving fixture on an active desktop. The two
excluded checks are unavailable NVIDIA execution and the separately reproduced
AMD AV1 geometry failure. The new same-frame fallback regression and
WGC compute synchronization test both pass. Report: `capture-native-final`.

Eight further 18-second streams repeated graphics/compute/compute/graphics at
idle and under the same GPU load, using production's `minimum_fps_target=20`
and the corrected 60-FPS animation on the unchanged 240-Hz monitor. Each cell
below averages two runs; five warmup seconds are excluded. Every picture
decoded, with 100% barcode coverage, nonzero test-tone audio, no capture
restart and no compute fallback.

| GPU condition | WGC copy/conversion | Host mean | Decoded picture-age mean | Distinct FPS | Transport FPS |
| --- | --- | ---: | ---: | ---: | ---: |
| Idle | Graphics | 2.533 ms | 14.789 ms | 59.965 | 59.965 |
| Idle | Compute | 2.247 ms | 11.455 ms | 60.001 | 60.001 |
| Loaded | Graphics | 15.326 ms | 52.859 ms | 51.423 | 56.331 |
| Loaded | Compute | 1.986 ms | 34.603 ms | 49.955 | 55.714 |

The idle runs all pass. All four loaded runs fail the unchanged 58.2-FPS
transport and freshness gates; decoded-picture correctness alone does not
make them performance passes. Compute reduces loaded picture age by 35%,
but distinct delivery also falls by about 3%. Its default activation was
therefore reverted. The implementation remains available for explicit testing;
the final default preserves the previous graphics-copy path. This is the third
rejected default, alongside notifications and the shorter encoder-output wait.
Do not choose only the favorable host-latency counter. Report:
`wgc-compute-abba3/results.json`, host SHA-256
`405f29d01c6ded76877ac8e4c2a3b0917a3172cf2c761201e3d1dbfd414259a9`.
The final build differs by disabling the default and making the native test
opt in explicitly. The report's per-run configuration identifies each path.

The retained release build has SHA-256
`c8341cefcb1cdfc50f8038e735c412175571222a743da5f0ba6ec6a388366959`.
All workspace binaries build; all-target Clippy with warnings denied,
formatting and diff checks pass. Its final workspace/native run again passes
200 checks, including 20 native checks, with the same two exclusions above.
Report: `retained-native-final`.

Final wired Intel VAAPI/readback runs use 60-FPS motion, the production repeat
floor and 2560×720 at 60 FPS / 20 Mbps. Every received picture decodes with
exact geometry, 100% barcode coverage, nonzero audio and no capture restart.
HDR remains conversion from an SDR desktop; it is not native HDR capture.

| WGC setting / codec | Decoded pictures | Transport FPS | Distinct FPS | Decode failures |
| --- | ---: | ---: | ---: | ---: |
| Default / HEVC | 1,077 | 60.000 | 59.922 | 0 |
| Compute opt-in / HEVC | 1,069 | 60.011 | 60.011 | 0 |
| Compute opt-in / HEVC Main10 HDR | 1,070 | 60.000 | 60.000 | 0 |

Removing the motion fixture deliberately fails motion validation despite
724/724 decoded pictures, zero codec errors and 60.606 transport FPS. The
receiver therefore cannot count a successful static decode as this motion
test's success. No remote picture-age result is emitted because clocks are
independent. Report: `retained-lan-final`. Test-owned processes and receiver
containers are stopped; the installed rc.2 service and its profile are intact.

### Startup diagnosis

The old `ddx_arrival_probe` inserted a zero latency sample when it captured
nothing, incorrectly printing one frame. It now reports actual acquisitions
and separate presentation samples. `ddx_startup_probe` compares raw BGRA-first,
FP16-first and legacy duplication with plain snapshots, configured graphics
and compute capture, and WGC. It records metadata and sparse pixel ranges,
without saving desktop pictures or changing display modes.

With Windows explicitly reporting the display off, all six DDX paths returned
zero frames; WGC returned one cached image. Continuous display requests,
including a separate system-plus-display request, did not wake this already
off output. The active-desktop DDX check passes. This narrows the earlier
standalone failure to an off-display condition on this machine; the request
still prevents sleep during an already active stream, as measured above.

During a cold stream, a separate raw DDX observer saw the output switch from
5120×1440 BGRA to 3840×2160 FP16 and back, with an access-loss event at each
switch. The configuration had display mode/HDR changes disabled. The trace
identifies the transitions, but not what initiated them. A subsequent warm
stream had no restart. DDX now logs its actual dimensions, format and API at
every open, and preserves the modern-API failure when legacy fallback occurs.
No fixed startup delay or black-pixel heuristic was added.

The Linux receiver also verifies the barcode's increasing frame sequence.
It deliberately omits absolute picture age because the remote clock is not
synchronized with Windows QPC.

## October 5 rc.3 selection and capture recovery

The user selected AMD WGC compute as the rc.3 default after reviewing the
production-repeat comparison above: about 35% lower loaded picture age with
about 3% fewer fresh pictures. The historical default-reversion record above
is retained. rc.3 enables `wgc_compute_copy` by default, retains the independent
off-switch and same-frame graphics fallback, and separates shared captures
when their compute settings differ. This does not change the backend selection
policy or make WGC available under SYSTEM.

Capture recovery previously slept 150 ms before every reopen, assuming all
streams had released the old GPU device. It now publishes a reset generation,
wakes consumers, and waits for acknowledgements after encoder, image and
filter teardown. Departing subscriptions stop blocking recovery; new
subscriptions own no old resources. Reopening starts once all owners release,
with 150 ms backoff only on failed open attempts and a 30-second deadline.
Tests cover multiple consumers, a departing/joining consumer, successive
resets and reset notifications arriving before or during a wait.

The tester can run a candidate when available, but the RX 9070 XT itself is
not remotely accessible. Keep the 4.7 versus 3.9 ms report open until matched
measurements arrive. The local fixture is RX 7900 XT and the LAN receiver is
a wired Intel NUC, not the reported Wi-Fi system. rc.3 artifacts are under
`%USERPROFILE%\.codex\artifacts\butterpollo-rc3-20261005`.

## October 5 AV1 idle recheck

The rc.5 follow-up initially recorded 56.6–57.5 fresh AV1 pictures per second
at a 60 FPS target. A subsequent GPU check found Warhammer 3 using 92–99%
of the graphics engine. After the user closed it, the same released rc.5
executable (`9c3bb28832e68cada56d637142b2e8b2527b745c6e9c6224db29cdc5500806a8`)
passed the idle motion check: 59.843 fresh FPS, 60.004 steady received FPS,
and all 1,047 received frames decoded without errors. Steady decoded picture
age averaged 10.562 ms (p95 11.512 ms, p99 12.620 ms); host time averaged
1.907 ms. The earlier loaded measurements remain valid observations, but
are not evidence of an idle AV1 regression or an RTSS cap failure.

This local check used an RX 7900 XT, WGC's user helper with both compute
paths enabled, a 2560×1440 physical desktop at 120 Hz, a 60 FPS moving strip,
and a 1280×720 AV1 stream at 20 Mbps. Overlay process lifetime was not recorded
throughout this check. A low-priority, two-job build was also running. The updated harness
lets the source renderer finish and retains its frame timestamps alongside
the receiver's barcode and timing records. Picture age stops at decode,
excluding client scanout and input latency. Evidence is under
`%USERPROFILE%\.codex\artifacts\butterpollo-monitor-av1-20261005\rc5-idle-av1-strip`.

The rc.6 runtime delivered 59.925 fresh FPS in the same idle strip check,
decoding all 1,116 received frames. With RTSS explicitly started and verified
alive through a separate rc.6 check, all 1,054 received frames decoded; the
steady window contained 752 distinct frames with no repeats or skips, at 60.001
fresh FPS. Decoded picture age averaged 13.795 ms (p95 14.383 ms), and host
time averaged 1.796 ms. The existing RTSS global profile remained at 120/1 FPS
with SyncLimiter=1; the test requested no limiter changes. RTSS was stopped
afterward to restore its prior process state. Optional codec detection also
completed with the same capability flags as rc.5. These checks do not claim an
rc.6 latency gain.

## October 5 rc.8 capture polling and RTSS audit

The WGC freshness predictor now caches its median and polling window when a
new frame updates the history. Polling no longer sorts two identical arrays
between frames. A regression test compares the cached decisions against the
previous calculation across stable and changing cadence, short intervals,
duplicate timestamps and capture resets.

Seven alternating local release-mode microbenchmarks each ran two million
polls, with one observation every eight polls and identical output checksums.
Median time fell from 84.79 ms to 14.32 ms (5.92 times faster for this small
calculation). Evidence and both implementations are under
`%USERPROFILE%\.codex\artifacts\butterpollo-rtss-autostart-20261005\qa`.

The first rc.8 candidate was installed through the normal update transaction.
Settings, pairings and app-library hashes remained unchanged. With RTSS fully
closed, the service retried Windows error 740 using the signed-in administrator,
applied 2997/50 FPS, and restored 120/1 FPS on disconnect while Desktop remained
retained. A 120 FPS renderer measured 59.941 FPS during the cap and 119.996 FPS
afterward. WGC compute remained active and all 831 AV1 frames decoded. Warhammer
3 was running, so these are functional checks with a background game, not an
idle comparison. A preceding run recovered from two display-resolution changes;
its disrupted timing result was rejected.

Fault injection also reproduced a reconnect bug: after failed SDK restoration,
replacing a retained process owner with `None` killed RTSS during the next lease.
The owner now survives a successful recovery and reacquisition. A separate
fixture reproduced missed process detection for a directory ending in `\.`;
directory identity is now normalized before comparison.

## October 5 rc.8 idle physical-display investigation

After Warhammer closed, the desktop was 5120×1440 SDR at a configured 240 Hz.
These checks stream 2560×720 at 20 Mbps through WGC's user helper and compute,
with independent local Moonlight/FFmpeg decoding and source frame barcodes.
They are not directly comparable to the earlier 1280×720 checks on a smaller
desktop. No compiler runs alongside the motion checks. The source logs and
DXGI presentation statistics distinguish requested refresh, actual presented
frames and distinct pictures reaching the decoder.

An initial alternating AV1 strip comparison gave 59.978 and 60.002 fresh FPS
for the first rc.8 build, versus 60.005 and 54.415 for the audited build. Three
subsequent audited runs delivered 59.912–59.938 fresh FPS; HEVC and H.264
checks gave 59.920 and 59.866. The failed run remains part of the evidence. A
full-screen pattern reproduced the shortfall in both builds: 50.092 and 49.437
fresh FPS, respectively. Source submission and displayed-present counters
confirmed approximately 60 FPS in the separate presentation probe. The frame
gaps appeared before encoding. Configured 240 Hz alone is not evidence that
every image was scanned out at 240 Hz or that VRR caused the gaps.

The C++ host sets WGC's minimum update interval to 1 ms; the Rust port had left
it at Windows' default. A [firsthand Windows capture
report](https://github.com/robmikh/Win32CaptureSample/issues/82) describes a
similar ceiling near 60 FPS with values below 1 ms. The
[MinUpdateInterval property](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.minupdateinterval)
is available on newer Windows builds. Local Windows 11 build 26200 accepted
the setting. An unconditional experiment measured:

| WGC configuration and source | Received FPS | Fresh FPS | Host processing mean | Decoded picture age mean |
| --- | ---: | ---: | ---: | ---: |
| Windows default, full-screen 120 FPS | 59.758 | 59.758 | 1.884 ms | 17.805 ms |
| 1 ms interval, full-screen 120 FPS | 100.326 | 94.180 | 1.853 ms | 10.171 ms |
| 1 ms, compute disabled, full-screen 120 FPS | 98.832 | 91.705 | 2.161 ms | 11.167 ms |
| 1 ms interval, 60 FPS strip | 60.516 | 54.120 | 1.874 ms | 18.920 ms |

Every received frame decoded without errors in these cases, but none passed
the strict fresh-picture target. The 1 ms experiment also failed a full-screen
60 FPS check (43.951 fresh FPS). It is therefore not enabled globally. The
first rate-aware candidate requested it only when the negotiated stream rate
exceeded 60 FPS, preserved the previous 59.94/60 FPS behavior, and kept compute enabled. The effective
policy follows the user helper and capture-sharing key. An explicit
`wgc_high_rate_capture=true` or `false` overrides the automatic decision;
unsupported Windows versions retain their default. The improvement at 120
FPS removes an observed ceiling, not all capture loss.

The final rate-aware executable was then compared with its own override
disabled, avoiding a binary-version confound. At a 120 FPS request it delivered
90.598 fresh FPS with the automatic 1 ms request, versus 59.959 with the
override disabled. Decoded picture age averaged 14.640 versus 10.247 ms;
this pair establishes higher delivery rate, not a uniform latency gain.
At 60 FPS the unchanged default delivered 57.178 and 47.846 fresh FPS, and
the preceding audited executable then delivered 47.567 under the same strip
test. Forcing 1 ms at 60 FPS gave 55.417 fresh FPS; combining it with fixed
grid pacing gave 56.521, with higher picture age (25.701 and 26.533 ms).
Neither diagnostic passed the 58.2 fresh FPS acceptance gate, and neither
became a 60 FPS default. No failed run was removed to claim success.

The source's displayed-present counter remained near 60 FPS in the strip
checks, while its reported refresh-counter rate differed (approximately 240 in
the first run and 100 in the later revised/old pair). The physical display
mode, driver settings and the user's applications were not changed by these
tests.

H.264 and HEVC HDR output from the SDR source reproduced the shortfall
(48.893 and 46.342 fresh FPS). Both decoded every received frame with correct
geometry and nonzero decoded audio. Disabling the optional DXGI statistics
in the source renderer still gave 48.528 fresh AV1 FPS, so removing that
instrumentation did not resolve this occurrence. The ordinary workspace
suite passed 206 tests (23 hardware/network tests ignored), all-target Clippy
passed with warnings denied, and the release workspace built successfully.
These code checks and successful decoding do not override the failed
fresh-picture acceptance results.

During diagnosis, an allocator-reused image address exposed a stale
`first_seen` trace entry. Clearing it after submission fixes the diagnostic;
it does not change pacing. `motion_probe` now optionally records DXGI
presentation statistics with `BUTTERPOLLO_TEST_PRESENT_STATS=1`.
`wgc_arrival_probe` adds an `unregistered` polling mode for comparison with
registered notifications; no production notification policy was changed.

All original runs, including failed comparisons, are preserved in
`%USERPROFILE%\.codex\artifacts\butterpollo-monitor-av1-20261005`. The
consolidated `CAPTURE_QA.json` is under
`%USERPROFILE%\.codex\artifacts\butterpollo-rtss-autostart-20261005\qa`.

## October 5 rc.8 installer firewall repair

The two manual installation attempts at 22:19–22:20 Berlin time copied the
rate-aware host successfully, then failed when `netsh` rejected the
`\\?\C:\Program Files\ButterpolloRust\butterpollo.exe` application path.
The preceding automatic updater had persisted its canonical filesystem path
in the Windows installation entry; the manual installer reused that path.
This failure happened after the motion benchmarks and cannot explain their
loopback fresh-frame loss. An existing private/domain, local-subnet allowance
remained, and the wired receiver at 192.168.4.10 could still reach serverinfo.

Setup now converts conventional drive/UNC paths at Windows command and
registration boundaries while retaining canonical filesystem identity checks
inside the updater. Unsupported namespaces and names that would change meaning
are rejected before firewall operations. Existing Rubylight rules are updated
in place; a missing rule is added. No existing rule is deleted on a failed
replacement, and legacy-rule cleanup follows a successful Rubylight rule.
Regression tests cover canonical paths with spaces and Unicode, UNC paths,
real-file identity, existing/fresh rules, and failure without deletion.

## October 5 rc.8 explicit WGC interval correction

A native background probe isolated WGC from the encoder, texture copies and
network. Each case opened a new capture session, left the first second out,
and read `MinUpdateInterval` plus frame timestamps. Warhammer remained open;
the probe created no visible window, changed no display/input/RTSS settings,
and saved no pictures. On Windows build 26200, the untouched property returned
160,000 100-ns units: **16 ms**, not zero.

| Fresh session setting | Property value | Native capture updates/sec |
| --- | ---: | ---: |
| Untouched, first control | 16 ms | 55.384 |
| Explicit zero | 0 ms | 216.968 |
| Explicit 1 ms | 1 ms | 216.569 |
| Untouched, final control | 16 ms | 57.493 |

A separate comparison rebuilt the previous capture implementation from
`ba784a5dba6b063605f22dc1691028ae2e76307e` and the corrected source with the
same build command. Two alternating three-second checks of direct WGC plus
the production GPU-copy path measured 59.489/58.000 capture updates/sec before
and 246.923/234.999 after, excluding each first second. These short checks ran
against the existing game picture without a test window or encoder. They
confirm removal of the capture ceiling, not a whole-stream FPS improvement.
The prior saved probe lacked source provenance and was excluded as a baseline;
its measurements remain in the artifacts. Exact-source results and binary
hashes are in `qa\explicit-zero-exact-source-comparison\results.json`.

This identifies a throttle before encoding and explains why simply omitting
the API call can miss a 60 FPS target. The low-rate path now explicitly sets
zero; the higher-rate path retains its 1 ms request. Both direct capture and
the user helper use the same constructor. Unsupported API versions keep the
existing nonfatal fallback. Capture sharing and compute-copy policy are
unchanged. `wgc_high_rate_capture=false` now selects explicit zero, rather than
leaving the property untouched.

Earlier failed barcode checks remain failed historical evidence. A controlled
motion check of this revised executable is still required before claiming the
full smoothness acceptance gate passes. The probe and timestamp records are
under
`%USERPROFILE%\.codex\artifacts\butterpollo-rtss-autostart-20261005\qa\native-cadence-fresh-session.json`.

Two further eight-second background AV1 checks used the real user-helper and
compute path at 2560x720, 60 FPS and 20 Mbps, with an independent local decoder.
They captured the existing game picture without a test window, audio tone,
input, display changes or frame limiter. After the five-second warmup, the
short measured windows were 2.102/2.146 seconds. The previous build delivered
58.039 FPS and missed the unchanged 58.2 FPS delivery gate; the corrected build
delivered 60.585 FPS and passed. All 412/435 received frames decoded, with zero
errors. Intervals over 1.5 frame periods fell from one to zero in those windows.
The final host samples reported mean source-frame ages of 0.681/3.419 ms and
mean present-to-send times of 2.526/5.252 ms; this is not evidence of reduced
latency. The game was uncontrolled and the sample was brief. Distinct-picture
cadence and sustained latency still require the controlled motion check.
The raw cases are `butterpollo-monitor-av1-20261005\wgc-zero-background-before`
and `wgc-zero-background-after`; the comparison is preserved separately from
the failed historical motion checks.

Verification of this revision: 208 ordinary tests passed, 24 environment tests
were skipped by default, the new native WGC interval regression passed when
selected explicitly, Clippy passed with warnings denied, and the release
workspace built successfully. The running game's process/start time and all
three installed host profile hashes were unchanged after the background tests.
After these measurements, the user installed the final candidate. Read-only
checks confirmed the tested host hash, service version 2.0.0-rc.8, successful
setup completion, a conventional installation path and an enabled inbound
firewall allowance for that executable. This confirms installation and service
startup; it does not substitute for a complete limiter lifecycle, controlled
motion check or automatic-update handoff.

## October 6 rc.9 WGC-first test candidate

An unset, blank or Automatic capture setting now resolves to WGC before the
stream opens capture. This uses the existing signed-in user helper when running
as a service, together with the existing Desktop Duplication fallback at startup
and recovery. Explicit capture choices retain their previous normalized value.
The initial candidate preserved compute-copy defaults, capture intervals,
stream rates, display refresh policy and frame limiting. The follow-up below
removes its automatic high-rate WGC interval limit.

The selection regression matrix covers physical/virtual displays, fractional
and integer frame rates, VRR, frame generation and legacy capture aliases.
The fallback regression starts from the default policy and injects a WGC open
failure, verifying that Desktop Duplication is attempted next. These are policy
and error-path checks, not new hardware or performance measurements.

Initial-candidate verification: 210 ordinary tests passed; the default run skipped 23 hardware
checks and one network check. Formatting, Clippy with warnings denied, the
release workspace build, and the web build passed. The web type check reported
zero errors and warnings. These checks ran without starting capture or changing
the installed service.

After the game closed and the user made the screen available, eight controlled
full-screen motion cases compared Automatic WGC/helper/compute with explicit
DDX on the unchanged 5120x1440, 240 Hz display. Each independently decoded
AV1 stream requested 2560x720 at 20 Mbps for 20 seconds, with a five-second
warmup and 14.3-14.8 measured seconds. The fixture rendered at the requested
stream rate; its recorded cadence was approximately 60 or 120 FPS. The test
used isolated loopback profiles, without display/RTSS changes or an audio tone.

| Stream target | Capture | Delivered FPS, two runs | Distinct-picture FPS, two runs |
| --- | --- | --- | --- |
| 60 | Automatic WGC | 60.602 / 60.586 | 56.203 / 57.495 |
| 60 | DDX | 60.594 / 60.591 | 54.454 / 53.844 |
| 120 | Automatic WGC, 1 ms | 107.272 / 109.162 | 100.344 / 101.478 |
| 120 | DDX | 121.196 / 121.206 | 113.908 / 113.850 |

All frames decoded without errors, all barcodes were readable, and the expected
backend opened without capture recovery or compute fallback. None of these
eight cases met the unchanged 97% distinct-picture gate. Fixed-grid WGC pacing
made 120 FPS delivery worse (99.271 FPS, 93.097 distinct); direct WGC without
the helper also failed (106.636 delivered, 100.157 distinct). These results do
not justify changing the default pacing mode or blaming the user helper.

Two follow-ups changed only the isolated profile's WGC interval override to
explicit zero. The helper/compute path then delivered 121.213/121.190 FPS,
with no intervals above 1.5 frame periods, versus 232/227 long intervals in
the default 1 ms cases. Distinct-picture rates rose to 116.274/116.663 FPS:
one narrowly missed the unchanged 116.4 FPS gate, and one passed. Mean decoded
picture age was 9.301/9.255 ms versus 8.983/9.230 ms with 1 ms, so this is a
delivery improvement, not evidence of reduced picture age. Mean host processing
remained about 1.85 ms. The revised default uses explicit zero at every rate;
an explicit wgc_high_rate_capture=true retains the 1 ms diagnostic option.

The candidate remains unpublished and is not installed over the running
service. The rebuilt executable was then checked twice without an interval
override: 121.186/121.193 delivered FPS, no long intervals and no decoding
errors. Distinct-picture rates were 116.289/116.595 FPS, so one still narrowly
missed 116.4 while the other passed. Mean decoded picture age was 9.084/9.339
ms; mean host processing was 1.864/1.865 ms. The test binary's SHA-256 is
f4a04b4634a690a86727b5b9c9a6da27ff558e275251fae2fc2a23400ef4c82b. The revised
source passed all 210 ordinary tests, with the same 24 environment checks
skipped, plus formatting, Clippy with warnings denied and the release workspace
build. Frontend sources did not change after their successful checks. No failed
case is relabelled as a pass. Logs, renderer reports, independent per-frame
CSVs and comparison summaries are retained under
D:\\CodexArtifacts\\butterpollo-wgc-default-20261006 alongside the initial
candidate package and its exact-source hashes.

## October 6 offline latency audit

Analysis of the same final two 120 FPS WGC runs, without new capture or changes
to the installed host, separates the latency counter from picture age:

| Stage or outcome | Run A / Run B |
| --- | --- |
| Host processing (claim to before packetization) | 1.864 / 1.865 ms mean |
| Capture timestamp to encoder claim | 1.378 / 1.548 ms mean; 4.616 / 5.119 ms p95 |
| Independent software decoder callback | 3.254 / 3.371 ms mean |
| Render timestamp to decoded picture | 9.084 / 9.339 ms mean |
| Distinct-picture rate | 116.289 / 116.595 FPS |

The software decoder cost is specific to the test fixture, not Moonlight Qt's
hardware decoder. These values exclude client display scanout and input latency.
At 60 FPS the WGC frame-claim wait averages roughly 4 ms, making frame selection
and pacing a stronger next investigation than further encoder preset changes.
Shortening that wait is not automatically beneficial: selecting an older picture
can worsen picture age even while lowering a host counter.

All fourteen source fixtures recorded render submissions, but none enabled the
existing optional DXGI actual-presentation statistics. The next comparison should
enable those statistics and trace picture identity through capture and encoding
before attributing the remaining repeated pictures to either stage. Compute
versus graphics copies also need a comparison using the corrected zero interval.
The exact workload and distinct-picture acceptance threshold should stay fixed.

Static review found two additional hypotheses, with no measured gain yet: drain
the two-buffer native WGC pool to the newest frame before copying after a stall;
and compare the helper's process scheduling settings with the host's streaming
scope under load. The helper already uses MMCSS and a high-resolution timer.
No capture, pacing, encoder or scheduling defaults were changed by this audit.

## October 6 capture phase and actual-presentation investigation

The follow-up measurements use a different desktop configuration: 2560x1440 at
120 Hz, streaming 1280x720 AV1 at 20 Mbps on the local RX 7900 XT. The display
was already in this mode when testing resumed. These ages must not be compared
directly with the earlier 5120x1440/240 Hz, 2560x720 results. Each controlled case
requested 20 seconds with a five-second warmup, WGC through the signed-in user
helper, compute conversion and the explicit zero capture interval. The receiver
independently decodes pictures in software on loopback; this is not a Moonlight
hardware-decoder or input-to-display measurement.

The fixture now records actual DXGI presentation statistics as well as render
submissions. Healthy source windows maintained the intended presentation rate
and a constant one-frame in-flight count. An additional presentation-to-decoder
estimate uses
`SyncQPCTime + (PresentRefreshCount - SyncRefreshCount) * QPCfrequency / refreshHz`
to estimate display presentation, then subtracts render-to-presentation delay
from the original decoded picture age. The fixture's present-call and picture
IDs are checked against the actual presentation counters before using that
mapping. The [DXGI statistics definitions](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_frame_statistics)
and [present-call counter](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgiswapchain-getlastpresentcount)
are distinct. This estimate supplements the original render-age metric; it does
not replace raw acceptance or include scanout, network Wi-Fi delay or input.

Trace-level capture records identify each publication, including publications
replaced before an encoder claim. Current instrumentation scopes IDs by source
and records generation, capture/publication timestamps and matching claims.
Weak identity records do not keep image textures alive, and tracing disabled
avoids populating those records.

Actual presentation was healthy in the initial four compute/graphics cases,
while the later 60 FPS diagnostic cases published roughly 70-78 capture updates
per second. Every decoded repeat in those cases had a new capture publication
ID. Thus a new captured image could contain the same source picture and consume
pacing credit immediately before the next useful update. In the baseline trace,
252 publications were replaced before claim, 85 claimed pictures repeated and
76 source picture IDs were absent. Disabling prediction reduced repeats to eight
and omitted source IDs to zero, but increased estimated presentation-to-decoder
age from 12.334 to 15.438 ms. A fixed-grid comparison likewise reached 60 distinct
FPS but increased that age to 19.275 ms. These alternatives do not justify a
global pacing change.

The opt-in `frame_pacing_source_phase=true` diagnostic learns a dominant phase
from capture timestamps observed by the streaming thread. It does not consume
fixture barcodes, render IDs or DXGI statistics. It requires a concentrated
rolling history and a recent source interval close to the stream interval;
irregular, faster and slower sources fall back to ordinary arrival pacing.
Recovery, timestamp discontinuities and persistent phase shifts reset history.
Anticipation remains bounded rather than adding a full source period. The
ordinary arrival policy and `frame_pacing_source_phase=false` remained defaults
during the initial diagnostics below.

Nine cases used the same runtime SHA-256
`1551d2f33a927023c5319f45d770cb07c3962c0ebed5374685f1066be6306c4a`.
All decoded without errors. Source and stream rates are independent in the last
four cases. Same-rate gates remain 58.2 distinct FPS at 60 and 116.4 at 120.
The explicitly planned 120-to-60 and 30-to-60 workloads use
`min(source FPS, stream FPS) * 97%`, recorded as receiver thresholds 58.2 and
29.1 respectively. A 30 FPS source cannot supply 60 distinct pictures. These
new mixed-rate cases have their own declared workload; historical failures and
same-rate gates remain unchanged.

| Case | Source to stream FPS | Delivered FPS | Distinct FPS | Original render age, mean ms | Estimated presentation age, mean / p95 ms | Wire gaps | Fresh-picture gaps |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| phase-60-a | 60 to 60 | 60.392 | 59.906 | 12.007 | 9.281 / 17.259 | 0 | 7 |
| base-60-b | 60 to 60 | 60.479 | 54.232 | 17.533 | 13.067 / 25.075 | 13 | 104 |
| phase-120-a | 120 to 120 | 120.133 | 119.927 | 14.315 | 6.043 / 7.795 | 0 | 3 |
| base-120-b | 120 to 120 | 120.823 | 119.930 | 14.904 | 6.633 / 9.954 | 0 | 13 |
| phase-60-b | 60 to 60 | 60.137 | 59.931 | 16.082 | 7.875 / 11.700 | 0 | 3 |
| phase-fast-source | 120 to 60 | 60.478 | 60.478 | 13.408 | 5.135 / 7.527 | 0 | 0 |
| base-fast-source | 120 to 60 | 60.577 | 60.577 | 13.308 | 5.034 / 7.237 | 0 | 0 |
| phase-slow-source | 30 to 60 | 49.109 | 29.973 | 17.652 | 15.269 / 32.405 | 233 | 0 |
| base-slow-source | 30 to 60 | 45.803 | 30.009 | 21.152 | 13.520 / 31.483 | 260 | 0 |

Wire gaps exceed 1.5 stream periods; fresh-picture gaps exceed 1.5 periods at
the available distinct-picture rate. The 30-to-60 cases have zero fresh-picture
gaps above 50 ms despite many wire gaps above 25 ms. Omitted source IDs for
120-to-60 can represent expected downsampling and are not alone a loss diagnosis.

Both 60 FPS phase runs passed their freshness gate and reduced estimated
presentation age by 3.785/5.192 ms relative to the intervening baseline, which
still failed freshness. The 120 FPS pair retained about 119.93 distinct FPS and
improved estimated age by 0.590 ms. The faster-source pair was effectively
unchanged, with phase 0.100 ms slower in this sample. The slower-source pair
was 1.749 ms slower after presentation with phase, despite a lower original
render age: the latter was influenced by different render-to-vsync phase. Both
retained all 438 distinct source pictures. Exact-estimator offline replays of
each saved slow trace selected identical capture IDs and times with phase
on/off across three encoder-busy assumptions, and observed claim/deadline
analysis found no changed phase deadline in those slow windows. Phase had more
surplus capture publications and repeated pictures.

Initial synthetic-load and DDX follow-ups used runtime SHA-256
`12bf588d1517a3ce4545366567bdcd11c0a55af4710cc0f43fd4becd572a1294`.
The four loaded cases kept the same source/stream rates and freshness gates;
a separate GPU workload rendered at roughly 120 FPS with about 5.1 ms median
and 5.9 ms p95 render work. The DDX pair used the controlled source without that
extra load. All six decoded without errors and maintained the intended average
source presentation rate. The loaded 60 FPS phase source occasionally alternated
one/three display refreshes around its intended two-refresh cadence; its baseline
source was uniform. Both loaded 120 FPS sources were uniform. These are synthetic
stress samples, not gameplay validation.

| Case | Distinct FPS | Original render age, mean ms | Estimated presentation age, mean / p95 ms | Wire gaps | Fresh-picture gaps |
| --- | ---: | ---: | ---: | ---: | ---: |
| load-60-base | 57.089 | 18.278 | 13.959 / 21.480 | 0 | 51 |
| load-60-phase | 59.310 | 22.708 | 14.390 / 23.263 | 1 | 18 |
| load-120-phase | 118.751 | 19.930 | 11.667 / 15.832 | 13 | 45 |
| load-120-base | 119.178 | 16.626 | 8.354 / 12.111 | 0 | 28 |
| ddx-60-phase | 58.284 | 17.155 | 12.600 / 21.747 | 0 | 34 |
| ddx-60-base | 55.083 | 20.413 | 12.304 / 21.415 | 0 | 81 |

Phase improved 60 FPS freshness under synthetic load and with DDX, but their
estimated presentation ages increased by 0.432 and 0.296 ms respectively. The
loaded 120 FPS phase run was 3.313 ms slower and had more long intervals than
baseline, although both passed freshness. Its source render-to-presentation
phase matched baseline, so that difference is not removed by presentation
normalization. That phase run had 103 capture publication intervals above 12.5
ms versus zero in baseline, even though actual source presentation was uniform.
Five of its 13 long wire intervals claimed a late publication immediately;
eight used the ordinary prediction deadline. None directly matched a phase
override on the claimed frame, but 76 other first deadlines did. The optional
capture publication alignment to the claim grid was disabled in these profiles;
shared GPU scheduling can still couple capture and encoder work.

A subsequent guard restricts phase overrides to sustained surplus:
at least 32 observed timestamps, using up to 64, must average at least 110% of
the requested stream rate. It counts timestamp intervals, resets with phase
history and falls back when the surplus ends. Fixed-trace replay preserved the
phase choices of the successful 60 FPS runs while selecting ordinary pacing on
the idle 120 FPS traces and loaded 120 FPS baseline trace. It reduced, but did
not eliminate, activation on the prior loaded 120 FPS phase trace. This replay
uses modeled newest-seen observations and several encoder-busy assumptions;
it does not reproduce shared GPU scheduling or optional claim-grid feedback to
capture. That replay alone did not establish runtime acceptance; the following
group measures the guarded implementation with explicit on/off settings.

### Guarded A/B, stress and compatibility checks

All thirteen cases in `guard-plan.json` used runtime SHA-256
`f0974b404dcc613cbeab0a19857bab5e8328c4c7aaeb1a919954c5fae63fabab`,
the same 2560x1440/120 Hz desktop and 1280x720/20 Mbps AV1 workload. The loaded
120 FPS group and 30-to-60 group use baseline/phase/phase/baseline order. The
final pair deliberately removes the synthetic GPU workload's rate cap; it
renders about 183.6-183.7 FPS with 5.4 ms median GPU work, leaving very little
headroom. The freshness gates remain 58.2, 116.4 and the explicitly planned
29.1 for the 30 FPS source. All thirteen cases decoded without errors.

| Case | Phase | Source / stream FPS | Distinct FPS | Original render age, mean ms | Estimated presentation age, mean / p95 ms | Wire / fresh / publication gaps |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| guard-idle60-1 | on | 60 / 60 | 60.000 | 12.628 | 7.902 / 12.358 | 0 / 3 / 0 |
| guard-idle60-2 | off | 60 / 60 | 57.650 | 19.269 | 11.847 / 19.194 | 0 / 43 / 0 |
| guard-idle60-3 | on | 60 / 60 | 59.862 | 16.671 | 8.221 / 15.647 | 0 / 4 / 0 |
| guard-load120-1 | off | 120 / 120 | 118.986 | 19.223 | 10.952 / 14.779 | 0 / 32 / 0 |
| guard-load120-2 | on | 120 / 120 | 119.118 | 16.813 | 8.541 / 12.392 | 0 / 29 / 0 |
| guard-load120-3 | on | 120 / 120 | 119.789 | 14.934 | 6.660 / 9.963 | 0 / 16 / 0 |
| guard-load120-4 | off | 120 / 120 | 118.593 | 19.455 | 11.186 / 15.003 | 0 / 38 / 0 |
| guard-slow-1 | off | 30 / 60 | 30.057 | 19.705 | 14.350 / 31.642 | 223 / 0 / 292 |
| guard-slow-2 | on | 30 / 60 | 29.992 | 19.548 | 14.655 / 30.423 | 172 / 0 / 245 |
| guard-slow-3 | on | 30 / 60 | 30.038 | 19.535 | 13.158 / 31.095 | 237 / 0 / 315 |
| guard-slow-4 | off | 30 / 60 | 30.025 | 19.266 | 14.187 / 31.573 | 270 / 0 / 329 |
| guard-saturated60-1 | on | 60 / 60 | 53.955 | 31.880 | unavailable | 27 / 110 / 64 |
| guard-saturated60-2 | off | 60 / 60 | 53.019 | 31.676 | unavailable | 28 / 120 / 61 |

Wire and publication gaps exceed 1.5 stream periods; fresh gaps use 1.5 periods
of available distinct content. Thus slow-source wire/publication intervals above
25 ms are kept in the table, while fresh content has no gaps above 50 ms. The
saturated cases no longer pass the presentation estimator's steady-source-rate
validation, so their adjusted ages are withheld. Their raw render ages and failed
freshness gates are retained. Actual source presentation averaged 59.691/59.140
FPS and varied between one and four display refreshes, rather than a uniform two.

The two guarded idle 60 FPS runs pass freshness and reduce estimated presentation
age by 3.945/3.625 ms versus the intervening baseline, or 3.785 ms averaging those
two run differences. The baseline still fails freshness. First-deadline trace
reconstruction confirms phase-specific decisions in both guarded runs. The loaded
120 FPS group reproduces neither the earlier long wire intervals nor long capture
publication intervals. However, neither enabled loaded run has a reconstructed
surplus activation in its steady window, so their lower measured ages must not be
credited as a new 120 FPS algorithmic latency gain. These are descriptive short
runs, not a statistical guarantee against all scheduling variation.

All four slow-source runs retain every available source picture and use
ordinary pacing in the reconstructed steady windows. Their mean estimated
presentation ages average 14.269 ms baseline and 13.907 ms guarded. Both
saturated runs fail the unchanged 58.2 FPS gate. Phase has no learned stable
center in those reconstructed windows, so it falls back, while host processing
remains about 1.80 ms. Pacing cannot be claimed to restore 60 distinct FPS
under this uncapped workload, and no game performance or remote RX 9070
XT/Wi-Fi conclusion follows from it.

The current candidate now enables guarded source-phase pacing for WGC-selected
streams; `frame_pacing_source_phase=false` preserves the previous behavior.
Explicit DDX keeps its previous default, since this guarded runtime group did not
validate DDX. This selects the measured 60 FPS improvement while retaining the
rate, concentration and recovery fallbacks. The default-path and recovery checks
below follow this decision; packaging is separate. These measurements do not
assert installation over the service or publication.

Runtime-4 verification passed 242 ordinary tests with 26 environment-dependent
tests ignored by default, and Clippy with warnings denied. Four selected native
checks separately passed across `native-3` and `native-4`: WGC reconnect/COM
teardown, H.264's negotiated one-reference budget, native H.264/HEVC/AV1 reference
recovery after dropped packets, and exact localized GPU texture comparison.
The one-reference H.264 check decoded all 64 frames with SPS and VUI reference
budgets of one and no LTR recovery; the unrestricted recovery test retained its
separate LTR coverage. The first default-selection follow-up repeated all 242
ordinary passes, 26 default skips, Clippy and the release build successfully.

Official Windows Moonlight Qt 6.2.0 also passed ten local hardware-decoder and
D3D11-renderer connections against runtime-3, SHA-256
`12bf588d1517a3ce4545366567bdcd11c0a55af4710cc0f43fd4becd572a1294`: two each
for H.264, HEVC, AV1, HEVC HDR negotiation and AV1 HDR negotiation at
1280x720/60 FPS. Requested formats matched negotiation, client logs reported
about 60 decoded/rendered FPS and no decoder errors, and each connection and
reconnect closed cleanly. Host cancellation returned the host to its free
state.

### Repeat deadlines and current candidate

The first default-enabled executable, SHA-256
`cbc837071d1b27a02050a3db1f16ec18aa444e54b4ecacd49c33b24534491ca6`,
passed the no-override AV1 60/120 FPS cases with 59.977/119.858 distinct FPS,
zero long wire intervals and zero decode errors. Original render ages were
15.407/14.791 ms mean, and estimated presentation ages were 8.185/6.520 ms.

A deliberately terminated WGC helper reopened in 309 ms after releasing encoder
resources in 31 ms. The HEVC stream resumed without decoder errors or a new
client connection. This is a functional recovery pass, not a smoothness pass:
its whole steady window produced 51.551 distinct FPS and failed the normal 58.2
gate. The recovery runner's explicitly recorded 50 FPS delivery threshold did
not replace that freshness gate. The shortfall already existed before failure
(51.189 distinct FPS in the preceding steady interval) and remained after the
first two recovery seconds were excluded (53.804). The outage itself skipped
19 source pictures, rather than explaining the entire deficit.

That profile requested `minimum_fps_target=60`. Frequent source presentation
intervals of one/three refreshes around the intended two caused old images to
reach their 16.667 ms repeat deadline before the useful update arrived. Repeating
them spent pacing credit. The old trace logged only new-image submissions, so
its 1,062 claim records could not be joined ordinally to 1,167 received frames.
Clock-aligned matching associated 81 of 111 steady repeated pictures with outputs
without a fresh-image claim record. Separate HEVC on/off/on cases then achieved
60.004/53.533/59.963 distinct FPS and estimated presentation ages of
7.312/11.840/7.417 ms, establishing that this was not a general HEVC decoding or
phase-pacing regression. The failed recovery smoothness result remains retained.

The follow-up gives a repeat a bounded opportunity to yield to an imminent new
capture when source-phase pacing is enabled and the observed source interval is
near the stream period. Extra wait cannot exceed one quarter of the stream
period plus 0.5 ms. Slower sources, disabled phase policy and non-arrival pacing
retain their existing repeat policy. Tracing now includes every submission with
an explicit `fresh` flag, allowing same-image repeats to be separated from a
newly captured image containing the same pixels.

Eight cases in `repeat-plan.json` compare the revised executable, SHA-256
`d22e446dd103592c09cd9bf11da3a76d355ab1783a18862d5d3b9f8b31a3237a`,
with the preceding executable at the same HEVC settings. All seven revised cases
pass their declared freshness gates and decode without errors; the old HEVC case
still fails 58.2. Source presentation is uniform within each measured window.
The normal-minimum cases use the actual default `minimum_fps_target=20`; the
high-minimum and slow-source cases are explicitly different workloads.

| Case | Codec | Source / stream / minimum FPS | Distinct FPS | Original render age, mean ms | Estimated presentation age, mean / p95 ms | Same-image repeats | Wire / fresh gaps |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| repeat-hevc-on1 | HEVC | 60 / 60 / 60 | 59.793 | 11.292 | 7.343 / 11.288 | 0 | 0 / 6 |
| repeat-hevc-old | HEVC | 60 / 60 / 60 | 57.700 | 16.713 | 13.269 / 22.959 | 42 inferred | 0 / 42 |
| repeat-hevc-on2 | HEVC | 60 / 60 / 60 | 59.860 | 10.317 | 7.210 / 11.157 | 0 | 0 / 5 |
| repeat-av1-60 | AV1 | 60 / 60 / 60 | 60.000 | 13.428 | 7.321 / 9.092 | 0 | 0 / 0 |
| repeat-av1-120 | AV1 | 120 / 120 / 120 | 119.603 | 15.608 | 7.334 / 11.421 | 1 | 0 / 23 |
| repeat-slow | HEVC | 30 / 60 / 60 | 30.025 | 29.907 | 22.907 / 37.708 | 305 | 1 / 5 |
| repeat-normal60 | AV1 | 60 / 60 / 20 | 59.976 | 12.522 | 8.027 / 12.570 | 0 | 0 / 4 |
| repeat-normal120 | AV1 | 120 / 120 / 20 | 120.006 | 14.906 | 6.633 / 10.219 | 0 | 0 / 10 |

The revised HEVC runs remove the same-image repeats observed in the matching
old case and lower estimated presentation age by 5.926/6.059 ms. This overlaps
with the pacing improvements above and must not be added to them as a separate
end-to-end gain. Repeated pixels in new capture publications still occur. The
30 FPS source with a forced 60 FPS repeat minimum delivers 60.600 encoded FPS,
retains every source picture, and has five fresh-picture intervals above 50 ms;
it is not a zero-jitter pass. Its age cannot be compared directly with the earlier
30 FPS tests using minimum 20. No same-image submission in the new traces decoded
as a different picture, supporting the trace's identity distinction.

The revised source passed 246 ordinary tests, with 26 environment checks
ignored by default, plus Clippy with warnings denied and the release build. New
tests cover bounded repeat waits and fallback conditions. Subsequent direct-WGC
and helper-failure checks on this same executable completed as follows;
installation and publication remain separate steps.

Direct WGC without the user helper passed 59.930/120.001 distinct FPS at requested
60/120 FPS, with zero decoder errors, long wire intervals, capture restarts or
compute fallbacks. Original render ages averaged 14.310/14.315 ms and estimated
presentation ages 7.738/6.041 ms. These cases retain the normal minimum of 20 FPS.

The final HEVC helper-failure check repeated the original explicit 60 FPS
minimum. Resource release took 31 ms and WGC reopened in 317 ms; the client
remained connected and decoded all 1,157 received frames without errors.
Including the forced outage, the whole steady window delivered 58.691 FPS and
58.348 distinct FPS, passing the unchanged normal 58.2 freshness gate. It
contains one 379.469 ms wire gap, five repeated pictures and 24 skipped source
pictures. Nineteen skipped pictures span the outage itself; five occur during
immediate encoder recovery. The preceding steady segment produced 59.343
distinct FPS; after the first recovery second, 59.996 distinct FPS resumed with
no omitted source pictures. Whole-window render age averaged 9.781 ms and
estimated presentation age 7.224 ms. Actual source presentation was uniform in
this run. Exact records are `direct-results.json`,
`final-helper-recovery2/recovery-result.json` and
`final-recovery-analysis-20261006/RECOVERY2_SEGMENTS.json`.

Three further official Qt 6.2.0 checks against the preceding
`cbc837071d1b27a02050a3db1f16ec18aa444e54b4ecacd49c33b24534491ca6` executable
verified the client's AV1 hardware-decoder, cropping and D3D11-renderer
metadata path: 1920x1080 at 8-bit and 10-bit, and 1968x2184 at 8-bit. The
client reported cropping coded 1920x1082 to 1920x1080 and 1984x2186 to
1968x2184, with no decoder errors and decoded/rendered rates near the requested
60 FPS. This shows the official client's handling of padding; it does not
change the failed raw elementary-bitstream geometry gate. Pixel readback, edge
correctness, native HDR color and display accuracy were not checked.

Raw plans, per-frame receiver CSVs, source presentation reports, acceptance
results and runtime provenance are under
`D:\CodexArtifacts\butterpollo-latency-compat-20261006`. The
`phase-case-analysis-20261006/RESULTS.json` and `COMPARISON.json` retain all nine
cases and raw gates; `stress-phase-review-20261006/RESULTS.json` retains the six
follow-ups. `guard-case-analysis-20261006` retains the complete thirteen-case
comparison and deadline reconstruction; `qt62/cases/official-fixed-3/summary.json`
retains the exact-client compatibility matrix. `repeat-case-analysis-20261006`
and `final-recovery-analysis-20261006` preserve repeat and recovery attribution.
`trace-review-20261006` and `slow-phase-review-20261006` preserve earlier trace
attribution and replay limitations. No result here validates the remote
RX 9070 XT/Wi-Fi report, sustained gameplay, native HDR accuracy or client
scanout. Recovery coverage is limited to the specific checks described above;
later codec fixes require their own exact-binary validation.

## October 6 follow-up: saturation, HDR state and compatibility

These follow-ups use evidence under
D:\CodexArtifacts\butterpollo-remaining-20261006. The first rc.10 test host is
SHA-256 93a681c26f8e767212e3580260d641658e1d871257b64aadd97b511e0e31bcbf. Its
ordinary suite passed 257 tests with 27 environment-dependent tests ignored by
default; Clippy with warnings denied and the release build passed.

The final hotplug-protection revision passed 263 ordinary tests with 27 excluded
by default, including six new hotplug tests. Formatting, Clippy with warnings
denied and release builds passed. Its host SHA-256 is
3b6d6e3299cc57383c4ca5aa373a0bfbb721218afb44114c81dbbdd61a9bd89c.
The earlier executable hash and SDR result below identify their own tested
build; final package and installation provenance are recorded separately.

### Saturation diagnosis and queue-drain rejection

Four serial cases ran the unchanged rc.9 executable
d22e446dd103592c09cd9bf11da3a76d355ab1783a18862d5d3b9f8b31a3237a
in baseline/drain/drain/baseline order. The only case setting changed was
wgc_drain_to_newest=false/true/true/false. All used the WGC helper and compute
copy, implicit default source-phase pacing, AV1 1280x720 at 60 FPS and 20 Mbps,
the 2560x1440 physical desktop at 120 Hz, minimum_fps_target=20, a 20-second
stream with five-second warmup, and the same bounded uncapped synthetic GPU
workload. The workload rendered at 184.5–184.6 FPS in all four cases.
The freshness floor remained 58.2 FPS.

| Case | Actual source presentation FPS | Distinct FPS | Original render age, mean ms | Estimated presentation age, mean ms | Wire gaps above 25 ms | Host processing, mean ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| rc9-saturated-base-a | 58.727 | 54.063 | 31.034 | 17.251 | 52 | 1.760 |
| rc9-saturated-drain-a | 59.518 | 52.632 | 28.355 | 15.252 | 79 | 1.757 |
| rc9-saturated-drain-b | 59.689 | 54.101 | 27.212 | 15.405 | 58 | 1.761 |
| rc9-saturated-base-b | 59.691 | 54.808 | 29.457 | 16.888 | 34 | 1.760 |

All four decode without errors, capture restart or compute fallback, but all
fail the unchanged distinct-picture floor. Drain-a also fails the delivery
floor at 57.304 FPS. Draining reduces picture age by roughly 1.5–2.0 ms while
losing more fresh pictures and increasing long wire intervals. It remains
disabled by default. The result must not be represented as a saturation fix.

The newer timing analysis checks DXGI presentation identity separately from
uniform requested cadence. Source frame IDs match Present call IDs, and actual
presentation remains one frame behind the calls throughout each measured
window. Those checks permit the same presentation-QPC estimate described above
even though the source's frame intervals are irregular. The older table's
withheld saturated estimates are retained as the original analysis; a separate
reanalysis estimates 17.860/17.757 ms for guard-saturated60-1/2. Neither the new
estimate nor this reanalysis changes a raw age or acceptance gate. This
approximation excludes input and display scanout and has roughly 0.1 ms
claim-clock alignment precision.

The measured encoder/host processing stays near 1.8 ms. Under saturation, both
capture publication delay and frame-selection delay rise. In the current
baseline cases, all 68/72 omitted source pictures have an estimated matching
capture publication. With drainage, 73 of 100 and 51 of 80 omitted pictures no
longer have an estimated matching publication. This is inferred identity for
unclaimed images: selecting the most recent actual presentation at least
0.25–1 ms before the capture timestamp agrees with every known claimed
picture in these four runs, but unclaimed pixels were not read back.

Fixed-publication replay does not support disabling prediction globally or
shortening the minimum claim spacing as a default fix. An ideal content
deduplication oracle improves modeled baseline freshness from 54.057/55.066
to 56.646/58.287 FPS at the existing spacing, but assumes zero GPU comparison
cost and uses picture identity unavailable to production. It retains source
irregularity and does not pass every case. Exact GPU comparison therefore
remains a diagnostic candidate, not a default capture optimization.

Raw cases and unchanged acceptance are in saturation/saturation-results.json.
RC9_ABBA_REPORT.md and RC9_ABBA_DIAGNOSIS.json in that directory retain stage
timing and provenance; RC9_ORACLE_AND_LIFETIMES.json retains the model's
assumptions. No game, display mode, installed profile or service was changed by
these four tests; the installed host was checked idle and Warhammer absent
before each run.

### HDR-state correction and current SDR validation

The candidate reads the explicit modern HDR capability/request bits and active
color mode, using the dedicated HDR setter rather than treating any Windows
Advanced Color state as HDR. Its ABI is checked against
[Microsoft's SDK definitions](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/um/wingdi.h).
Legacy query/set behavior is retained for operating systems without the modern
query; a failed modern setter does not fall back to changing WCG. A successful
request must settle in both requested and active state. Temporary query errors
are retried; a timed-out owned request is rolled back while preserving an
observed competing request. Unit checks include pending-state cancellation,
failed disable rollback and explicit rollback failure.

The actual physical display path exposed modern flags 69 with HDR-support bit 4
clear and active mode SDR, while the legacy value 5 reported general Advanced
Color/WCG capability. The corrected probe refuses to claim native HDR from this
state. Earlier attempted physical HDR color results that used BGRA8 capture are
not native HDR validation.

One rc.10 AV1 SDR check, rc10-sdr-av1-720p60-1, passed on the unchanged physical
desktop using default WGC/helper/source-phase behavior and minimum 20 FPS.
All 1,172 received frames decoded without errors. Its steady window delivered
60.002 FPS and 871 consecutive distinct source pictures, with no repeats,
omitted IDs or wire gaps above 25 ms. Host processing averaged 1.815 ms and
original render-start-to-decoder age averaged 9.508 ms. DXGI presentation
counters remained zero in this smoke, so actual presentation timing is
unavailable; the raw age must not be relabeled as presentation-to-decoder
latency. The decoder barcode gate is independent of those missing counters.
Physical settings and 120 Hz refresh were unchanged after cleanup.

The final host repeated this default-path SDR check in
rc10-sdr-av1-720p60-2. All 1,169 received frames decoded; the steady window
delivered 59.999 distinct FPS with 868 consecutive unique picture IDs, no repeats,
omitted IDs or long wire gaps. The physical mode, 120 Hz refresh and SDR state
were unchanged. DXGI statistics again supplied no usable presentation-ID/QPC
pairs, so this is freshness and compatibility evidence without an actual
presentation-to-decoder latency claim. The original 58.2 FPS floor was retained.

The initial native virtual HDR attempts below failed before pixel validation.
The reviewed isolated harness uses one
owned extended HDR display, full-window native 1280x720 motion, independent
frame readback and the original color thresholds: luma/chroma mean absolute
error at most three 10-bit code values, contrast 0.98–1.02 and saturation
0.95–1.05. The first normally elevated attempt failed the VDD access preflight
with AccessDenied before starting its private host, creating a display,
pairing, capture or encoding. Its failure is not a codec or HDR pixel failure.
No native HDR pass or relaxed threshold follows from it. A later native HDR result
requires its own recorded capture and pixel evidence.

Subsequent isolated service-context attempts did open WGC with RgbaF16 source
pixels. One stopped because ANSI log formatting obscured the harness's readiness
check; the corrected reader preserves all acceptance tokens and thresholds.
The next attempt detected a second newly active display, the previously dormant
HISENSE, alongside the owned HDR virtual display. The exact physical/topology
gate rejected it before motion began. The original Odyssey settings were
unchanged, and teardown restored the original topology in 2.327 seconds. Neither
attempt provides a native HDR color-roundtrip pass.

The source fix captures available inactive targets immediately before owned VDD
creation/recreation and protects those same identities through HDR startup. It
prunes only reactivated dormant targets from a fresh active topology, keeping
the supplied modes, path priority and clone relationships. Strict temporary
SetDisplayConfig flags neither save the topology nor allow Windows to retime
the supplied modes. The owned display is followed by driver target and monitor
path, so GDI display-name reuse cannot redirect its HDR setup or recovery.
A second semantic snapshot check rejects an observed concurrent layout change.
The final settle phase requires 500 ms of quiet within one 1.5-second deadline,
retained across retries; expiry prevents further mutation. Protection ends
after startup. Windows has no atomic topology compare-and-set, so a narrow
last-call race remains, and intentional activation of the same dormant target
during this short window cannot be distinguished from automatic topology recall.
Six pure tests cover the selection, clone/mode, identity, race and deadline
rules. The reporter's phone behavior has not been reproduced by these local
tests; final native pixel-validation evidence follows separately.

### Final native virtual HDR pixels, excluding physical-panel calibration

Two isolated service-context runs used the final host above, each with one owned
extended 1280x720 HDR virtual display, full-window 60 FPS motion, WGC helper
capture and AMF at 40 Mbps. The stream ran for 22 seconds with a five-second
warmup and minimum repeat target 20 FPS. Both reported active modern HDR and
RgbaF16 capture; independent software decoding supplied a ten-bit frame dump
at received frame 480. No capture fallback or restart occurred. These are
native HDR capture/conversion/codec pixel checks, not physical-panel calibration,
client HDR rendering, scanout or game-content validation.

| Codec | Received/decoded frames | Steady distinct FPS | Unique pictures in steady window | Repeats / skipped IDs | Wire intervals above 25 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| HEVC Main10 HDR | 1,281 / 1,281 | 60.001 | 985 / 985 | 0 / 0 | 0 |
| AV1 Main10 HDR | 1,302 / 1,302 | 60.026 | 1,000 / 1,000 | 0 / 0 | 31 |

| Codec | Luma MAE, 10-bit codes | Chroma MAE, 10-bit codes | Contrast slope | Saturation ratio |
| --- | ---: | ---: | ---: | ---: |
| HEVC Main10 HDR | 0.393 | 0.404 | 0.9995 | 1.0059 |
| AV1 Main10 HDR | 0.387 | 0.428 | 0.9983 | 0.9976 |

Both black patches decoded to code 64 and white patches to 509 against 509.08
expected. The original limits were retained: black error at most two codes,
white error at most three, luma/chroma MAE at most three, contrast 0.98–1.02
and saturation 0.95–1.05. Both passed the unchanged 58.2 distinct-FPS floor and
decoded without errors. No latency improvement is claimed from these two HDR
runs.

Each run passed 11 during-stream topology samples and a mandatory check before
cancellation, with the original physical settings intact and only the owned
virtual display added. Logs in both runs show the guard restoring the dormant
HISENSE target after creation and again after HDR settings. This validates
correction of that reproduced startup recall, not prevention of every transient
activation or the remote phone report. Final teardown removed the virtual
display and restored the original topology; the installed service stayed
running and its configuration hash was unchanged. The temporary dispatcher
tasks were removed. HEVC/AV1 complete fixture durations were 35.493/35.280 seconds,
including 0.401/0.439 seconds of cleanup.

Full results are system-context/hdr-9dae55fb791f48f2b23414231da970dc/
coordinator-result.json (HEVC) and
system-context/hdr-4edd39d9a9d441bb8f179768b3ee1d8f/coordinator-result.json (AV1).
Their matching hdr-virtual case directories retain raw host/receiver logs,
pixel dumps, color results, topology samples and before/after snapshots.

#### Repeat HDR runs and four-run totals

Two further runs repeated AV1 and then HEVC with the same final executable,
1280x720/60 native FP16 virtual display, 22-second duration, five-second warmup
and unchanged freshness/color gates. Together with the original pair above,
the recorded order is HEVC/AV1/AV1/HEVC.

| Repeat codec | Received/decoded frames | Steady distinct FPS | Unique pictures in steady window | Repeats / skipped IDs | Wire intervals above 25 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| AV1 Main10 HDR | 1,288 / 1,288 | 59.999 | 989 / 989 | 0 / 0 | 0 |
| HEVC Main10 HDR | 1,302 / 1,302 | 60.001 | 999 / 999 | 0 / 0 | 0 |

| Repeat codec | Luma MAE, 10-bit codes | Chroma MAE, 10-bit codes | Contrast slope | Saturation ratio |
| --- | ---: | ---: | ---: | ---: |
| AV1 Main10 HDR | 0.401 | 0.503 | 0.9984 | 0.9976 |
| HEVC Main10 HDR | 0.374 | 0.434 | 0.9995 | 1.0057 |

The AV1 repeat's decoded reference frame also verifies black and a 100-nit
white patch. The source renders white as 1.25 in linear scRGB, where 1.0
represents 80 nits. These are mean limited-range ten-bit PQ luma codes from
received frame 480 (source frame 468):

| Source patch | Expected Y′ | Decoded Y′ |
| --- | ---: | ---: |
| Black | 64.00 | 64.00 |
| 100-nit white | 509.08 | 509.00 |

The values come from the AV1 repeat's `colour-result.json` and describe one
reference frame. The four-run frame total below counts successful decoding.

Both repeats passed their independent pixel checks, all 11 during-stream
topology samples, the pre-cancel topology check and complete restoration.
The installed service remained running and its configuration hash was unchanged.
Across all four HDR runs, **5,173 received frames decoded successfully** with
zero decoder errors, and **44 during-stream topology samples passed**, plus
four pre-cancel checks. Adding the final SDR check's 1,169 decoded frames gives
**6,342 clean decodes across five rc.10 runs**. These are whole-run decode totals;
the fresh-picture counts in the tables cover only the steady windows.

The four independently decoded reference frames have luma MAE 0.374–0.401
and chroma MAE 0.404–0.503 in 10-bit code values. Both recorded mean-error
metrics stay below 0.51 code values; this describes those four pixel dumps,
not a per-pixel maximum or a color analysis of all 5,173 frames.

The first AV1 run's 31 long arrival intervals remain in the original table.
The repeat adds an observation with zero intervals above 25 ms without replacing
that uneven run or establishing universally even cadence. All source, scope
and physical-panel limitations above remain unchanged.

The rc.10 validation record (VALIDATION.json of the rc.10 release, since removed)
contains all four cases in current_validation.native_hdr.fixtures. Repeat raw
results are system-context/hdr-88d67d5ef35c450da7e4badb41d069d5/
coordinator-result.json (AV1) and
system-context/hdr-f87e34f8d65c44cfa13e2c8d4b7d5d52/coordinator-result.json (HEVC),
with full evidence in their matching hdr-virtual case directories.

### Native controller and Moonlight 6.2 command checks

One explicitly selected native HID test passed with the installed signed
libvirtualgamepad 0.1.0.39 driver. It sequentially created an owned neutral
DualShock 4 and DualSense and read their newly enumerated HID reports through
the production packet decoder and Gamepads::apply path. It verified two
independent primary-pad contacts, stable movement identity, release/reuse,
cancel-all, rejection of a third occupied contact, and removal on drop.
Secondary-surface events with matching pointer IDs did not alter either
primary contact. The first probe's VID/PID pathname assumption was corrected
to inspect HID attributes; production input behavior did not change.

This validates the existing fallback and primary-pad behavior, not a second
native touch surface. The signed driver's protocol has a contact index but
no independent surface index. A genuine second surface needs a compatible
device profile, versioned driver protocol and signed package; it cannot be
represented by the second finger of the primary pad. No live physical
two-pad controller, network-delivered touch event or game-specific mapping
was tested. Evidence is touchpad/INVESTIGATION.json and
touchpad/native-test-attempt2.log; the selected test executable SHA-256 is
5b01008db084c9aad9ed1cfa886b2b6c2a41772230b7187d4403420be9b031b0.

Further checks of the unchanged official Windows Moonlight Qt 6.2.0 executable
resolve part of the earlier command-line uncertainty. Against a private rc.9
software-only host, plain listing and cached CSV listing exited naturally
with code zero in 0.204/0.207 seconds. Three running-app quit cases exited
naturally in 3.847–3.849 seconds, and idle quit in 0.811 seconds. The fixture
used a verified private Windows desktop with normal GUI startup behavior:
the earlier hidden-startup flag suppressed the client's GUI polling, and its
half-second post-cancel wait was too short. No physical desktop switch or
user input was needed.

An unmodified official client still hangs on a cold-cache CSV listing in the
controlled mock case; the 65.143-second observation was stopped and is retained
as a failure. Source/debugger evidence associates it with asynchronous artwork
workers during Qt thread-pool shutdown. Plain listing or a normally populated
artwork cache avoids that tested path. No host protocol workaround or official
client patch was shipped. These command checks contain no RTSP stream and do
not replace the earlier codec matrix or validate the new candidate's codecs.
cli/SUMMARY.json preserves exact client/host hashes, mock controls and six
passing pure lifecycle tests.

An optional [Moonlight 6.2.0 source patch](compatibility/moonlight-6.2.0/README.md)
now makes CSV listing read existing artwork without starting background
artwork downloads. Eight focused regression cases pass against the actual
patched BoxArtManager translation unit with real Qt 6.4.2 on Linux; three
negative controls expose the original background-worker behavior. This is
source-only validation with stubbed HTTP/computer and cache boundaries, not a
full Windows client build or an end-to-end patched-client result. It does not
replace the official executable or erase its recorded cold-cache failure.

The read-only environment collector now records OS, GPU/driver, physical
adapter/link counters, host version/hash and service state without changing
settings or uploading data. Its local output is environmental evidence only.
Neither it nor these local tests reproduces the remote RX 9070 XT over Wi-Fi
report, establishes NVIDIA execution, or supplies physical-panel calibration
and client-scanout validation.

## October 7: rc.17 against rc.2 on the October 4 fixture

The [1080p at 60 fps](#1080p-at-60-fps) fixture again (`run-motion.py`
from `day-work-20261002`, copied with only its tool paths changed): RX 7900
XT, HEVC 10-bit HDR at 1920×1080 and 60 fps, 20 Mbps requested, native AMF
at ultra-low latency with `speed`, compute conversion, a 120 Hz virtual
display, and the same game-like load (`gpu_load 45 1000 0 200`) on the load
rows. The rc.2 binary is the one measured on October 4 (`5747318fae0a4581`),
rc.17 the installed release (`a8f491c3257431fe`). Each run started as SYSTEM
in the signed-in session, as the service starts the host: the virtual
display driver now refuses an administrator. Rows alternated in one batch,
three runs each. The DDX rows capture with Desktop Duplication as on
October 4; the WGC row is rc.17's default capture.

| Picture age, mean / p95 (ms) | rc.2, DDX | rc.17, DDX | rc.17, WGC |
|---|---|---|---|
| Idle | 14.69 / 15.77, 14.91 / 17.16, 14.62 / 15.47 | 14.78 / 16.00, 14.66 / 16.04, 14.69 / 15.93 | 14.93 / 16.22, 14.80 / 15.98, 14.84 / 15.91 |
| Load | 35.64 / 44.80, 35.19 / 44.59, 36.10 / 46.53 | 35.72 / 46.31, 35.01 / 43.63, 35.34 / 44.38 | 33.15 / 43.06, 33.34 / 43.70, 33.77 / 44.82 |
| Load, new pictures per second | 57.6, 58.8, 57.6 | 58.2, 59.7, 59.2 | 58.3, 58.1, 58.4 |
| Load, host latency | 5.79, 5.49, 5.68 ms | 5.20, 5.70, 5.53 ms | 1.87, 1.85, 1.86 ms |

On the same capture path rc.17 delivers the picture when rc.2 did, idle
(14.7 ms both) and beside the load (35.4 against 35.6 ms, within the
spread of the runs). With its default WGC capture it delivers it 2.2 ms
sooner beside the load than rc.2 (33.4 against 35.6 ms; 43.9 against
45.3 ms at the 95th percentile), and the host latency Moonlight reports
falls from 5.7 to 1.9 ms. Idle, all three are equal; every idle run
delivered 60 new pictures a second without a repeat.

rc.2 measured 35.6 ms beside the load here against 42.4 ms on October 4,
more than the difference between the versions: absolute values move
between batches, so only compare rows measured together. Beside the load
the probe rendered at 60-66 Hz instead of 120, as on October 4, so the
fixture's source-rate check fails on load rows as it did then; their
picture-age statistics are complete. One rc.2 idle run's probe exited
with an error after the stream had ended. Artifacts: `bench-rc17\cmp-*`.

rc.19 (`88e8fd322e8b1a6a`), three idle runs with WGC later the same day on
the same fixture: 14.95 / 16.49, 14.96 / 16.14 and 14.74 / 15.94 ms, 60.4
new pictures a second without a repeat, as rc.17. Artifacts:
`bench-rc17\rc19wgc-*`.
## October 7: PyroWave conversion on the compute queue

PyroWave's colour conversion (RGB to its Y, Cb and Cr planes) ran as three
D3D11 draws on the graphics queue. Beside the game-like load
(`gpu_load 30 1000 0 200`), `examples/performance.rs --codec pyrowave --hdr
--yuv444 --records` at 1080p, 120 fps and 400 Mbps measured 5.6-5.8 ms per
frame against 0.47 ms idle. PyroWave's own GPU work, from
`pyrowave_device_report_performance_stats`, was 0.11 ms idle and 0.19 ms
under the load; a CPU wait on the conversion's fence showed where the rest
went: 5.1 ms under the load against 0.15 ms idle.

The planes are now written by one compute shader pass on the D3D12 compute
queue the AMF path uses (`pyro_cs`, high priority), into the same textures
Vulkan imports, with the shared fence ordering the two queues. The planes
are byte-identical to the graphics pass for HDR and SDR, 4:4:4 and 4:2:0,
three output sizes and a composited pointer
(`compute_planes_match_the_pyrowave_graphics_planes`).

| Per frame, RX 7900 XT | Graphics queue | Compute queue |
|---|---|---|
| Idle, repeat-frame throughput | 0.47 ms | 0.47 ms |
| Beside the load, repeat-frame throughput | 5.62, 5.75 ms | 0.57, 0.54 ms |
| Beside the load, paced at 120 fps | 4.62 ms | 0.71 ms |
| Paced, bytes per frame idle / beside the load | 155,780 / 155,960 | 155,802 / 155,834 |

The fixture client does not decode PyroWave, so no PyroWave stream was
played end to end; equal planes and equal frame sizes show the encoder
reads the same pictures.
## October 7: two frames in the encoder

The stream claimed a picture whenever the pacer allowed one, and AMF only
pushed back at eight frames in flight. When one encode takes longer than a
frame interval, up to eight frames queued and each was that much older when
it came out, without any more frames per second for it. Since rc.19 a claim
waits while two frames are in the encoder, enough to keep both of a Radeon's
encoder instances busy, and the host takes the encoder's output first.

5120×1440 HEVC at 240 fps and 150 Mbps on the RX 7900 XT, whose encoder
manages 220 fps there; two alternating runs each:

| Encoder saturated | rc.18 | rc.19 |
|---|---|---|
| Present to send, mean | 42.7 ms | 11.1 ms |
| Present to send, p99 | 45.9 ms | 13.3 ms |
| Host latency | 39.6 ms | 8.9 ms |
| Frames per second | 220 | 220 |

Streams the encoder keeps up with are unchanged: H.264, HEVC, AV1 and HEVC
VRR at 2560×720 and 60 fps pass the release e2e.
## October 7: AMF low-latency mode and AV1 latency mode

`amd_lowlatency_mode` (AMF's `LowLatencyInternal`, H.264 and HEVC) and
`amd_av1_latency_mode` (`Av1EncodingLatencyMode`) default to `auto`, which
leaves the property unset. Whether forcing them on is worth a default was
measured with `examples/performance.rs --synthetic 16 --paced` at 2560×1440
and 120 fps, 20 Mbps, `speed`, `vbr_latency`, on the RX 7900 XT with driver
32.0.31041.1004. The probe reports submission to output; the settings line
the host logs shows what the driver applied.

With the default usage, ultra-low latency, the driver already applies both:
`LowLatencyInternal=1` and `Av1EncodingLatencyMode=3` (lowest) with the
properties unset. Forcing them changed nothing. Three alternating runs each:

| Ultra-low latency, mean / p95 (ms) | `auto` | forced on |
|---|---|---|
| H.264 | 3.12 / 3.58, 3.13 / 3.50, 3.08 / 3.40 | 3.19 / 3.64, 3.10 / 3.37, 3.09 / 3.46 |
| HEVC | 3.13 / 3.62, 3.17 / 3.64, 3.17 / 3.64 | 3.12 / 3.48, 3.16 / 3.62, 3.31 / 3.69 |
| AV1 | 2.61 / 3.11, 2.62 / 3.11, 2.62 / 3.12 | 2.66 / 3.11, 2.64 / 3.12, 2.65 / 3.13 |

Every forced run produced the same number of bytes as its `auto` pair, to
the byte. The other usages, one run each:

| Usage | Codec | Driver's choice | `auto`, mean / p95 | Forced on |
|---|---|---|---|---|
| Low latency | HEVC | `LowLatencyInternal=0` | 3.59 / 3.92 ms | 3.16 / 3.63 ms |
| Low latency | AV1 | mode 0 (none) | 3.89 / 4.28 ms | 2.64 / 3.13 ms |
| Low latency, high quality | HEVC | `LowLatencyInternal=1` | 3.50 / 3.87 ms | 3.58 / 3.83 ms |
| Low latency, high quality | AV1 | mode 3 (lowest) | 3.48 / 3.76 ms | 3.51 / 3.83 ms |
| Transcoding | HEVC | `LowLatencyInternal=0` | 3.43 / 3.76 ms | 3.04 / 3.34 ms |
| Transcoding | AV1 | mode 0 (none) | 3.92 / 4.22 ms | 2.60 / 2.95 ms |

Forcing them helps only with a usage that leaves them off: about 0.4 ms per
HEVC frame and 1.3 ms per AV1 frame here, at the same output size. Both stay
`auto`. A default of on would change nothing for the default usage and
override a user's deliberate choice of another one, and writing
`LowLatencyInternal` explicitly froze HEVC encoding on RX 9000 cards with
Adrenalin 26.5 (video stalls while audio plays), the reason the native AMF
encoder stopped forcing it. The setting descriptions now say which usages
already have them on. The fallback console page saved the low-latency switch
as `amd_low_latency_internal`, a key nothing reads; it now saves
`amd_lowlatency_mode` and offers the AV1 latency mode too.
## October 7: how much bitrate PyroWave needs

A user streamed PyroWave at 720p60 with Moonlight's bitrate at 10 Mbps and
saw blurred grey blocks: a fifth of a bit per pixel per frame leaves room for
only the coarsest brightness layers. To see what the encoder takes when the
bitrate does not limit it, `examples/performance.rs --codec pyrowave
--records --paced --bitrate 1000000` at 1920×1080 and 60 fps, four seconds
each, on the RX 7900 XT:

| Picture | Per frame | Bits per pixel | Mbps at 60 fps |
|---|---|---|---|
| Desktop, SDR 4:2:0 | 624 KB | 2.41 | 300 |
| Desktop, HDR 4:4:4 | 478 KB | 1.84 | 229 |
| Moving test picture (`--synthetic 16`), SDR 4:2:0 | 1,822 KB | 7.03 | 875 |
| Moving test picture, HDR 4:4:4 | 1,344 KB | 5.18 | 645 |

The PyroWave README targets "~200+ mbit/s" on a wired LAN. A 720p60 HDR
stream to Nonary's client at 149 Mbps (2.7 bits per pixel) worked, at 60 fps
and 0.68-0.78 ms from present to encoded frame.

The initial warning used one bit per pixel per frame, less than half of
what that desktop took: once when the stream starts and when the client lowers
the bitrate (`PyroWave has too little bitrate`), and on the console's stream
card. That is about 55 Mbps at 720p60, 125 Mbps at 1080p60 and 500 Mbps at
4K60 (`pyrowave::minimum_kbps`). The representative-picture measurements
below replace this initial threshold. The host does not raise the
bitrate itself: the client's choice may reflect its network.
## October 7: PyroWave decoded end to end

The ignored `pyrowave::decode_test::pyrowave_decoded_end_to_end` test sends
known pictures through `Encoder::new_gpu_options` and `encode_gpu`, codec
3, then decodes with the pinned PyroWave 0.6 SDK, bitstream `186f0393`.
The decoder has its own Vulkan device on the same AMD adapter. It writes
three imported D3D11 textures through `pyrowave_decoder_decode_gpu_buffer`;
a shared fence completes before D3D11 staging readback. SDR8 uses R8 UNORM,
and both ten-bit formats use R16 UNORM, retaining the decoder's precision.

The sender's `VideoPacketizer::encode_pyrowave` produces the datagrams,
including its normal 20% critical FEC for records, with optional detail FEC
disabled. A separate receive parser orders the data shards, removes the
transport header and final padding, then
feeds either the length-prefixed codec packets or the individual records
to `pyrowave_decoder_push_packet`. Record padding is skipped. The test
requires complete-frame readiness and rejects early readiness. This covers
complete, unencrypted delivery; it does not exercise network loss recovery,
socket pacing or client display.

Two deterministic charts contain colour bars, smooth gradients, one- and
two-pixel text strokes, and independent RGB noise between 25% and 75%.
An independent double-precision CPU reference converts the actual uploaded
pixels to full-range BT.709 SDR or BT.2020 PQ HDR, including the centred
2x2 chroma average for 4:2:0. SDR10 starts from FP16 scRGB, rather than
eight-bit pixels in a ten-bit container. HDR tests both FP16 scRGB with
1000-nit white and packed ten-bit PQ input. PSNR uses peaks of 255 or 1023;
maximum errors are in those code values, including fractional codes from
R16 readback. HDR PSNR is in PQ code space, not linear light.

Each case alternates graphics/compute, compute/graphics, graphics/compute.
It asserts that compute actually ran, and that every decoded sample is
identical across both conversions, both framings and all three batches.
The test resets only the encoder's previous timestamp before each sample,
using its normal first-frame budget of 1/60 second: CPU reference and
readback time cannot inflate the plain-packet budget. These are picture
quality measurements, not sustained throughput measurements.

The 400, 125 and 30 Mbps cases run at both sizes, along with a 2000 Mbps
control. At 2000 Mbps the test requires at least 40 dB on every plane and
35 dB on every chart panel; at 400 Mbps it requires 24 dB on every plane.
The control is needed because 400 Mbps still loses fine text and noise at
1080p, especially in 4:4:4. A record-framed bitrate is the wire total, so
1000 Mbps leaves 1.82 MB per 1080p frame and 4:4:4 scores 42.8/37.3/36.6 dB
(Y/Cb/Cr, SDR8); 2000 Mbps encodes 3.28 MB and scores 82.0/63.0/62.7 dB.
The first control ran at 1000 Mbps, before the bitrate became the wire total. These are regression limits for this chart,
not recommended quality targets or assertions that the codec is lossless.

On an AMD machine, use the usual Rust SDK environment, then run from the
workspace root. On this machine the environment script is
`%USERPROFILE%\.codex\artifacts\butterpollo-rust-20260930\performance-probe\rust-env.ps1`.
`BUTTERPOLLO_PYROWAVE_ROOT\share\pyrowave-shared\build-info.txt` must name
commit `186f0393b77f7755953b5ecde994bb1cec2e4155`.

```powershell
. "$env:USERPROFILE\.codex\artifacts\butterpollo-rust-20260930\performance-probe\rust-env.ps1"
cargo test -p butterpollo-windows --lib --no-run --target-dir target\qa
if ($LASTEXITCODE) { throw 'Test build failed' }
Copy-Item "$env:BUTTERPOLLO_PYROWAVE_ROOT\bin\libpyrowave-shared-0.dll" target\qa\debug\deps\
cargo test -p butterpollo-windows --lib pyrowave::decode_test::pyrowave_decoded_end_to_end --target-dir target\qa -- --ignored --exact --nocapture --test-threads=1 2>&1 | Tee-Object target\qa\pyrowave-decode.log
if ($LASTEXITCODE) { throw 'PyroWave decode test failed' }
```

`PYROWAVE_RESULT` lines contain JSON with per-plane PSNR and maximum error,
frame bytes and datagram counts. `panels` lists bars, gradient, text and noise
for each of Y/Cb/Cr; a null PSNR with zero maximum error means an exact match
(infinite PSNR). Before each batch, `PYROWAVE_BATCH` records the installed host
log's last connect/disconnect event, or notes that its log is absent. Check
those lines when comparing a machine shared with streams or other GPU work. The
test only reads that log and does not start a host or change its settings.

On October 7, 2026, the AMD Radeon RX 7900 XT with driver 32.0.31041.1004
passed all 1,152 decodes in 620.90 seconds in the debug test build. All 288
batch checks found the last stream event was a disconnect at 12:57:29 UTC.
Other GPU work was not controlled. The maximum compute/graphics difference
was zero storage units, including all 16 bits of the ten-bit output planes.
Both framings and all three repeats also matched exactly: repeat spread
was 0.00 dB PSNR and zero maximum-error difference for every picture.

Each row below gives the lower PSNR and larger maximum error of the two
pictures, separately for each plane. Repeated measurements do not change those
values. The maximum error uses 8-bit codes for SDR8 and 10-bit codes for
SDR10/HDR10; it is not a count of R16 storage units.

| Size | Format | Chroma | Mbps | PSNR Y/Cb/Cr (dB) | Max error Y/Cb/Cr (codes) |
|---|---|---|---:|---|---|
| 1920x1080 | SDR8 | 4:2:0 | 1000 | 80.62/81.16/80.35 | 1.0/1.0/1.0 |
| 1920x1080 | SDR8 | 4:2:0 | 400 | 33.34/43.05/42.06 | 64.0/36.0/27.0 |
| 1920x1080 | SDR8 | 4:2:0 | 125 | 22.34/28.37/29.01 | 132.0/93.0/124.0 |
| 1920x1080 | SDR8 | 4:2:0 | 30 | 18.64/23.33/26.13 | 186.0/114.0/129.0 |
| 1920x1080 | SDR8 | 4:4:4 | 1000 | 44.78/41.20/40.87 | 23.0/24.0/27.0 |
| 1920x1080 | SDR8 | 4:4:4 | 400 | 28.83/25.90/27.26 | 96.0/97.0/92.0 |
| 1920x1080 | SDR8 | 4:4:4 | 125 | 21.89/23.28/24.87 | 141.0/103.0/116.0 |
| 1920x1080 | SDR8 | 4:4:4 | 30 | 18.63/21.22/23.40 | 186.0/127.0/135.0 |
| 1920x1080 | SDR10 | 4:2:0 | 1000 | 67.65/68.04/67.97 | 3.1/3.0/3.0 |
| 1920x1080 | SDR10 | 4:2:0 | 400 | 33.32/43.35/42.19 | 258.8/145.5/97.4 |
| 1920x1080 | SDR10 | 4:2:0 | 125 | 22.32/28.42/29.00 | 528.8/374.7/492.3 |
| 1920x1080 | SDR10 | 4:2:0 | 30 | 18.63/23.36/26.14 | 745.0/456.2/517.5 |
| 1920x1080 | SDR10 | 4:4:4 | 1000 | 45.22/41.33/40.84 | 108.6/93.6/108.1 |
| 1920x1080 | SDR10 | 4:4:4 | 400 | 28.88/25.41/27.31 | 382.8/387.1/364.5 |
| 1920x1080 | SDR10 | 4:4:4 | 125 | 21.89/23.32/24.91 | 572.2/412.0/480.2 |
| 1920x1080 | SDR10 | 4:4:4 | 30 | 18.61/21.24/23.42 | 745.0/512.3/541.3 |
| 1920x1080 | HDR10 | 4:2:0 | 1000 | 68.12/67.87/66.92 | 3.1/2.7/2.9 |
| 1920x1080 | HDR10 | 4:2:0 | 400 | 39.49/49.67/49.24 | 155.1/79.7/34.4 |
| 1920x1080 | HDR10 | 4:2:0 | 125 | 28.93/35.42/39.77 | 294.4/144.5/83.2 |
| 1920x1080 | HDR10 | 4:2:0 | 30 | 23.49/31.17/37.98 | 466.2/176.4/101.9 |
| 1920x1080 | HDR10 | 4:4:4 | 1000 | 53.65/47.58/51.44 | 43.0/38.5/29.2 |
| 1920x1080 | HDR10 | 4:4:4 | 400 | 35.29/34.12/38.01 | 176.5/133.7/85.5 |
| 1920x1080 | HDR10 | 4:4:4 | 125 | 27.69/31.15/36.34 | 305.7/152.7/85.1 |
| 1920x1080 | HDR10 | 4:4:4 | 30 | 23.48/29.10/35.32 | 476.7/202.0/103.4 |
| 1280x720 | SDR8 | 4:2:0 | 1000 | 77.53/84.60/80.58 | 1.0/1.0/1.0 |
| 1280x720 | SDR8 | 4:2:0 | 400 | 65.69/79.82/76.16 | 2.0/2.0/1.0 |
| 1280x720 | SDR8 | 4:2:0 | 125 | 27.27/33.94/33.93 | 105.0/50.0/62.0 |
| 1280x720 | SDR8 | 4:2:0 | 30 | 19.92/24.09/26.26 | 186.0/103.0/116.0 |
| 1280x720 | SDR8 | 4:4:4 | 1000 | 77.53/62.48/62.88 | 1.0/2.0/2.0 |
| 1280x720 | SDR8 | 4:4:4 | 400 | 42.55/37.25/36.81 | 24.0/42.0/46.0 |
| 1280x720 | SDR8 | 4:4:4 | 125 | 26.04/23.86/25.89 | 109.0/97.0/105.0 |
| 1280x720 | SDR8 | 4:4:4 | 30 | 19.24/22.09/23.81 | 186.0/134.0/122.0 |
| 1280x720 | SDR10 | 4:2:0 | 1000 | 67.49/68.05/67.76 | 4.0/2.6/2.9 |
| 1280x720 | SDR10 | 4:2:0 | 400 | 65.96/68.01/67.74 | 6.5/7.2/4.8 |
| 1280x720 | SDR10 | 4:2:0 | 125 | 27.24/33.93/34.05 | 422.7/199.3/251.2 |
| 1280x720 | SDR10 | 4:2:0 | 30 | 19.91/24.20/26.28 | 745.0/410.3/470.4 |
| 1280x720 | SDR10 | 4:4:4 | 1000 | 67.49/62.36/62.34 | 4.0/5.5/5.2 |
| 1280x720 | SDR10 | 4:4:4 | 400 | 42.62/37.27/36.83 | 109.1/129.2/184.8 |
| 1280x720 | SDR10 | 4:4:4 | 125 | 26.04/23.89/25.92 | 434.2/387.2/408.1 |
| 1280x720 | SDR10 | 4:4:4 | 30 | 19.19/22.13/23.84 | 745.0/498.0/491.5 |
| 1280x720 | HDR10 | 4:2:0 | 1000 | 67.99/67.83/66.85 | 2.6/2.7/3.4 |
| 1280x720 | HDR10 | 4:2:0 | 400 | 67.99/67.83/66.85 | 2.6/2.7/3.4 |
| 1280x720 | HDR10 | 4:2:0 | 125 | 34.40/41.98/42.13 | 163.5/81.9/76.0 |
| 1280x720 | HDR10 | 4:2:0 | 30 | 25.07/31.56/38.60 | 458.5/191.8/87.4 |
| 1280x720 | HDR10 | 4:4:4 | 1000 | 67.99/61.99/61.41 | 2.6/5.6/6.8 |
| 1280x720 | HDR10 | 4:4:4 | 400 | 50.15/44.48/45.80 | 89.0/64.9/55.6 |
| 1280x720 | HDR10 | 4:4:4 | 125 | 33.18/31.73/37.17 | 288.2/144.2/86.9 |
| 1280x720 | HDR10 | 4:4:4 | 30 | 25.05/29.58/35.52 | 459.5/195.7/91.6 |

At the time of this chart test, `minimum_kbps` was 124,416 kbps at 1080p60
(one bit per pixel per frame).
The 400, 125 and 30 Mbps budgets are 3.215, 1.005 and 0.241 bits per pixel
per frame. At 720p60 the warning was 55,296 kbps, and the same budgets are
7.234, 2.261 and 0.543 bits per pixel per frame. These are nominal encoder
budgets; framing, FEC and network headers add bytes on the wire.

There is substantial detail loss below the warning, but these pictures do
not show a sharp quality cliff at one bit per pixel. In the 1080p SDR8
4:2:0 chart, the worst text-panel luma PSNR is 30.08 dB at 400 Mbps,
18.03 dB at 125 Mbps and 13.75 dB at 30 Mbps. The corresponding whole-plane
luma scores are 33.34, 22.34 and 18.64 dB. Quality is already poor at the
warning's boundary; the larger drop is between 400 and 125 Mbps.

That threshold was a conservative warning about very low bitrate, not a promise
of good quality above it. If it is intended to protect fine SDR text, about 3.2
bits per pixel per frame is a candidate for 4:2:0: 400 Mbps is the lowest
tested 1080p rate to keep both text and whole-plane luma above 30 dB. That
would be about 178 Mbps at 720p60 by pixel-count scaling, an estimate rather
than a measured cutoff. Only the 1000 Mbps control clears 40 dB on all planes
throughout this matrix. More rates near a proposed boundary and real
desktop/game captures are needed before choosing a replacement that applies
across content and chroma formats. The following representative picture sweep
replaces that provisional warning.

## October 7: PyroWave bitrate from representative pictures

The ignored `pyrowave::decode_test::pyrowave_quality_*` tests extend the
decoded end-to-end harness above. All pictures are generated in Rust; no
downloaded images or captures are used:

- Desktop: a light editor, coloured text, navigation, toolbars, a graph,
  selection changes and one-, two- or four-pixel font strokes at 720p,
  1080p and 4K respectively.
- Game: a sky gradient, clouds, a bright sun, stone and brick textures,
  foliage noise at several spatial scales, a crosshair and a health bar.
  The second picture translates the textures and foliage and moves clouds.
- Dark: a dim light gradient, low-contrast edges and moving fine noise.

Each scene runs at 1280x720, 1920x1080 and 3840x2160, in SDR8 and HDR10,
with 4:2:0 and 4:4:4, at nominal 30, 60 and 120 fps. Requested budgets are
0.25, 0.5, 0.75, 1, 1.5, 2, 2.5, 3, 3.2, 3.5, 4, 4.5 and 5 bits per pixel
per frame (bpp). Each combination has two pictures and two repeats. The
rate order alternates low/high budgets (0.25/5, 0.5/4.5, and so on), then
reverses for the repeat. Every batch records the installed host log's last
connect/disconnect event. Other GPU work is not controlled.

The path is the same full encoder, packetizer, independent receive parser,
Vulkan decoder and staging readback used above, with compute conversion,
record framing, 1392-byte packets, 20% critical FEC and no detail FEC.
The previous encoder timestamp is reset for every sample, so CPU work
cannot inflate the nominal frame budget. These are quality tests at those
budgets, not measurements of sustained frame rate, latency or a live client.

At 4K the normal record/FEC transport limit caps the actual encoder budget
at 3,921,592 bytes per frame (about 3.78 bpp). The requested 4, 4.5 and
5 bpp points therefore test the same cap, not those actual bit allocations.
The `pyrowave_quality_2160p_without_fec` control repeats 4, 4.5 and 5 bpp
with critical FEC disabled in the test configuration. Its larger transport
cap allows those budgets; the control uses the same scenes, formats,
frame rates, phases and alternating repeats. It does not change the
installed host or the normal stream defaults.
The sweep supplies bitrate directly to the encoder. Normal stream setup
also caps bitrate at 2 Gbps, and the runtime `/bitrate` endpoint caps it at
500 Mbps; this test does not raise either limit. A high requested rate in
the tables is not evidence that a client can negotiate or sustain it.

HDR uses 200-nit desktop/game white, a sun reaching 1000 nits, and a dark scene
below 20 nits. Each HDR scene has FP16 scRGB and packed ten-bit PQ input,
compared with the independent full-range BT.2020 PQ reference. SDR uses
full-range BT.709. PSNR is reported on each Y/Cb/Cr plane with peaks of 255 or
1023.

The simple SSIM diagnostic averages non-overlapping uniform 8x8 windows
(partial edge windows included), using population variance/covariance and
constants `(0.01 L)^2` and `(0.03 L)^2`. It has no Gaussian weighting,
multiscale processing or perceptual HDR weighting. HDR scores are in PQ code
space, not linear light, and are not directly comparable with SDR. Unit tests
cover identity, opposite constant pictures, ten-bit output scaling and inverted
structure.

The quality criteria are:

- Severe-loss floor: desktop and game luma PSNR at least 25 dB and luma
  SSIM at least 0.80 in every tested picture.
- Clean-picture recommendation: desktop and game PSNR at least 35 dB on
  **every** plane and luma SSIM at least 0.95 in every tested picture.

These are explicit engineering targets for these synthetic scenes, not
universal visual thresholds. The earlier chart remains a harder stress test.
AMD is the measured platform; NVIDIA users should use Vibepollo.

To reproduce, use the SDK environment and DLL from the preceding section:

```powershell
. "$env:USERPROFILE\.codex\artifacts\butterpollo-rust-20260930\performance-probe\rust-env.ps1"
cargo test -p butterpollo-windows --lib --release --no-run --target-dir target\qa
if ($LASTEXITCODE) { throw 'Test build failed' }
Copy-Item "$env:BUTTERPOLLO_PYROWAVE_ROOT\bin\libpyrowave-shared-0.dll" target\qa\release\deps\
cargo test -p butterpollo-windows --lib --release pyrowave::decode_test::pyrowave_quality_ --target-dir target\qa -- --ignored --nocapture --test-threads=1 2>&1 | Tee-Object target\qa\pyrowave-quality.log
if ($LASTEXITCODE) { throw 'PyroWave quality sweep failed' }
```

`PYROWAVE_QUALITY` JSON lines contain the scene, size, format, chroma, fps,
requested bpp/kbps, critical FEC percentage, actual byte budget, padded frame bytes, RTP datagram
bytes (including parity but excluding UDP/IP headers), and per-plane PSNR,
maximum code error and SSIM. Null PSNR with zero error means an exact match.
The existing chart test is still available separately.

### Measured results and warning levels

On October 7, the AMD Radeon RX 7900 XT, driver 32.0.31041.1004,
decoded all 5,616 comparisons in the complete three-size matrix.
The SDK was bitstream `186f0393`, with the three patches listed above.
All 216 batch checks found the last service event was `CLIENT DISCONNECTED`
at 15:51:34 UTC. Other GPU work was not controlled. Matched repeats
had a maximum spread of **0.00 dB PSNR, 0 SSIM and 0 frame bytes**.
At fixed bpp, 30/60/120 fps also produced identical scores and frame sizes.

The complete 1080p data is retained from the interrupted first sweep.
The subsequent 720p and 4K sweeps passed in 56.36 and 303.39 seconds
respectively in the release test build. All 1,551 comparable samples
from the interrupted 4K sweep matched the complete rerun exactly.
The tables use only one complete sweep per size. Raw JSON is in
`target/qa/pyrowave-quality.log` (1080p),
`target/qa/pyrowave-quality-720p.log` and
`target/qa/pyrowave-quality-2160p.log`.

Each cell below is **minimum PSNR across Y/Cb/Cr, in dB / minimum luma SSIM**,
over desktop and game pictures, both phases, all three frame rates and both
repeats. The two minima need not come from the same picture. At 4K, the 4–5 bpp
requests all reach the same 3.78 bpp budget cap. Decisions use unrounded
scores.

**1280x720: desktop and game**

| Requested bpp | SDR 4:2:0 | SDR 4:4:4 | HDR 4:2:0 | HDR 4:4:4 |
|---:|---|---|---|---|
| 0.25 | 15.71 / 0.4324 | 15.71 / 0.4324 | 23.68 / 0.6278 | 23.68 / 0.6278 |
| 0.5 | 16.53 / 0.5101 | 16.53 / 0.5101 | 24.41 / 0.6929 | 24.41 / 0.6929 |
| 0.75 | 17.67 / 0.5681 | 17.67 / 0.5681 | 25.88 / 0.7502 | 25.86 / 0.7498 |
| 1 | 19.66 / 0.6288 | 19.65 / 0.6287 | 27.28 / 0.7992 | 27.26 / 0.7989 |
| 1.5 | 23.49 / 0.7619 | 23.35 / 0.7555 | 31.18 / 0.9000 | 31.12 / 0.8981 |
| 2 | 24.84 / 0.8065 | 24.70 / 0.8049 | 32.50 / 0.9231 | 32.40 / 0.9211 |
| 2.5 | 27.12 / 0.8297 | 27.01 / 0.8249 | 35.86 / 0.9400 | 33.44 / 0.9307 |
| 3 | 30.46 / 0.8550 | 29.98 / 0.8466 | 39.41 / 0.9618 | 38.98 / 0.9572 |
| 3.2 | 32.57 / 0.8842 | 30.15 / 0.8500 | 40.56 / 0.9673 | 39.39 / 0.9616 |
| 3.5 | 34.35 / 0.9086 | 32.59 / 0.8850 | 41.28 / 0.9712 | 40.61 / 0.9677 |
| 4 | 36.09 / 0.9369 | 34.64 / 0.9147 | 45.03 / 0.9855 | 41.99 / 0.9750 |
| 4.5 | 39.91 / 0.9662 | 37.08 / 0.9488 | 47.07 / 0.9915 | 45.03 / 0.9855 |
| 5 | 41.82 / 0.9751 | 39.99 / 0.9672 | 48.56 / 0.9946 | 46.83 / 0.9907 |

**1920x1080: desktop and game**

| Requested bpp | SDR 4:2:0 | SDR 4:4:4 | HDR 4:2:0 | HDR 4:4:4 |
|---:|---|---|---|---|
| 0.25 | 18.48 / 0.6140 | 18.47 / 0.6134 | 26.37 / 0.7745 | 26.37 / 0.7746 |
| 0.5 | 19.78 / 0.7070 | 19.77 / 0.7068 | 27.56 / 0.8325 | 27.56 / 0.8325 |
| 0.75 | 20.05 / 0.7489 | 20.05 / 0.7474 | 27.86 / 0.8511 | 27.86 / 0.8507 |
| 1 | 21.69 / 0.7846 | 21.69 / 0.7825 | 30.56 / 0.8935 | 30.08 / 0.8875 |
| 1.5 | 28.56 / 0.8824 | 27.05 / 0.8668 | 36.40 / 0.9559 | 36.38 / 0.9517 |
| 2 | 30.77 / 0.9086 | 30.75 / 0.9073 | 38.78 / 0.9675 | 38.34 / 0.9628 |
| 2.5 | 32.82 / 0.9281 | 32.13 / 0.9233 | 40.36 / 0.9798 | 39.26 / 0.9678 |
| 3 | 37.33 / 0.9611 | 35.24 / 0.9487 | 44.70 / 0.9887 | 42.58 / 0.9809 |
| 3.2 | 38.92 / 0.9721 | 38.26 / 0.9653 | 46.17 / 0.9912 | 45.23 / 0.9894 |
| 3.5 | 42.39 / 0.9854 | 40.79 / 0.9780 | 49.92 / 0.9961 | 49.05 / 0.9948 |
| 4 | 44.82 / 0.9905 | 41.93 / 0.9889 | 53.68 / 0.9986 | 51.35 / 0.9962 |
| 4.5 | 45.85 / 0.9926 | 43.52 / 0.9906 | 57.79 / 0.9994 | 54.54 / 0.9986 |
| 5 | 49.69 / 0.9966 | 43.53 / 0.9925 | 64.56 / 0.9999 | 54.86 / 0.9993 |

**3840x2160: desktop and game**

| Requested bpp | SDR 4:2:0 | SDR 4:4:4 | HDR 4:2:0 | HDR 4:4:4 |
|---:|---|---|---|---|
| 0.25 | 22.33 / 0.7449 | 22.33 / 0.7442 | 30.03 / 0.8435 | 30.03 / 0.8436 |
| 0.5 | 23.65 / 0.7947 | 23.64 / 0.7932 | 31.48 / 0.8701 | 31.48 / 0.8698 |
| 0.75 | 26.22 / 0.8251 | 26.19 / 0.8061 | 34.95 / 0.8960 | 34.55 / 0.8894 |
| 1 | 32.13 / 0.8673 | 30.78 / 0.8469 | 38.28 / 0.9205 | 37.97 / 0.9136 |
| 1.5 | 35.93 / 0.9332 | 35.16 / 0.9188 | 40.46 / 0.9549 | 40.37 / 0.9539 |
| 2 | 37.57 / 0.9464 | 36.98 / 0.9416 | 42.49 / 0.9683 | 41.95 / 0.9648 |
| 2.5 | 39.42 / 0.9655 | 37.84 / 0.9491 | 45.22 / 0.9834 | 42.50 / 0.9684 |
| 3 | 43.07 / 0.9863 | 39.44 / 0.9656 | 49.11 / 0.9934 | 46.23 / 0.9869 |
| 3.2 | 44.73 / 0.9910 | 40.68 / 0.9747 | 50.88 / 0.9956 | 49.24 / 0.9936 |
| 3.5 | 45.31 / 0.9921 | 41.29 / 0.9849 | 51.96 / 0.9966 | 50.62 / 0.9956 |
| 4 | 45.48 / 0.9925 | 42.02 / 0.9910 | 56.18 / 0.9987 | 51.37 / 0.9962 |
| 4.5 | 45.48 / 0.9925 | 42.02 / 0.9910 | 56.18 / 0.9987 | 51.37 / 0.9962 |
| 5 | 45.48 / 0.9925 | 42.02 / 0.9910 | 56.18 / 0.9987 | 51.37 / 0.9962 |

**4K control: full 4–5 bpp budgets without critical FEC**

All 432 additional decodes passed in 84.64 seconds. All 72 batch checks
again found the last disconnect at 15:51:34 UTC. Repeat and frame-rate
spreads were zero for PSNR, SSIM and frame bytes. This brings the total
to 6,048 decoded comparisons. Raw JSON is in
`target/qa/pyrowave-quality-2160p-without-fec.log`.

Without critical FEC, the test can pass 4,147,196, 4,665,596 and 5,183,996
bytes to the encoder at 4, 4.5 and 5 bpp respectively. These values are
within four bytes of the requested budgets, below the 5,228,792-byte
transport budget cap. The following scores use the same desktop/game
minimum PSNR / luma SSIM convention as the main tables. These control
results are separate from the default-FEC results used for the warnings.

| Requested bpp | SDR 4:2:0 | SDR 4:4:4 | HDR 4:2:0 | HDR 4:4:4 |
|---:|---|---|---|---|
| 4 | 46.65 / 0.9940 | 42.03 / 0.9910 | 58.65 / 0.9992 | 51.37 / 0.9962 |
| 4.5 | 51.90 / 0.9981 | 43.60 / 0.9925 | 64.60 / 0.9999 | 54.70 / 0.9988 |
| 5 | 55.12 / 0.9991 | 44.25 / 0.9925 | 64.60 / 0.9999 | 55.80 / 0.9993 |

The dark-scene control scores (minima over both chroma formats) were:

| Requested bpp | SDR | HDR (PQ code space) |
|---:|---|---|
| 4 | 57.02 / 0.9998 | 58.12 / 0.9985 |
| 4.5 | 57.02 / 0.9998 | 63.18 / 0.9996 |
| 5 | 57.02 / 0.9998 | 63.18 / 0.9996 |

**Dark scene**

These are minimum Y/Cb/Cr PSNR / minimum luma SSIM across all three sizes,
both chroma formats, phases, frame rates and repeats. The small signal
range makes this scene a poor basis for a bitrate recommendation.

| Requested bpp | SDR | HDR (PQ code space) |
|---:|---|---|
| 0.25 | 48.72 / 0.9856 | 44.42 / 0.9628 |
| 0.5 | 49.04 / 0.9866 | 44.78 / 0.9657 |
| 0.75 | 49.27 / 0.9872 | 45.01 / 0.9674 |
| 1 | 49.44 / 0.9877 | 45.17 / 0.9687 |
| 1.5 | 50.37 / 0.9901 | 46.58 / 0.9774 |
| 2 | 51.59 / 0.9926 | 47.84 / 0.9832 |
| 2.5 | 52.52 / 0.9940 | 48.94 / 0.9871 |
| 3 | 53.23 / 0.9949 | 50.09 / 0.9901 |
| 3.2 | 53.68 / 0.9954 | 50.68 / 0.9913 |
| 3.5 | 56.07 / 0.9974 | 52.27 / 0.9939 |
| 4 | 56.70 / 0.9993 | 55.02 / 0.9968 |
| 4.5 | 56.70 / 0.9993 | 55.02 / 0.9968 |
| 5 | 56.70 / 0.9993 | 55.02 / 0.9968 |

The lowest tested budgets meeting each criterion, also passing at every
higher tested budget, were:

| Size | Format | Chroma | Floor bpp | Clean bpp |
|---|---|---|---:|---:|
| 720p | SDR8 | 4:2:0 | 2.5 | 4.5 |
| 720p | SDR8 | 4:4:4 | 2.5 | 5 |
| 720p | HDR10 | 4:2:0 | 1.5 | 3 |
| 720p | HDR10 | 4:4:4 | 1.5 | 3 |
| 1080p | SDR8 | 4:2:0 | 1.5 | 3 |
| 1080p | SDR8 | 4:4:4 | 1.5 | 3.2 |
| 1080p | HDR10 | 4:2:0 | 0.5 | 1.5 |
| 1080p | HDR10 | 4:4:4 | 0.5 | 1.5 |
| 2160p | SDR8 | 4:2:0 | 0.75 | 2.5 |
| 2160p | SDR8 | 4:4:4 | 0.75 | 3 |
| 2160p | HDR10 | 4:2:0 | 0.25 | 1.5 |
| 2160p | HDR10 | 4:4:4 | 0.25 | 1.5 |

Use **2.5 bpp for the floor and 5 bpp for the recommendation below 1080
lines**, and **1.5 / 3.2 bpp at 1080 lines and above**. Keep the same
levels for SDR/HDR and 4:2:0/4:4:4. The 1080p levels are conservative
at 4K. This avoids treating the 720p one-pixel desktop text as if it
were the two- or four-pixel text in the larger pictures. At 720p, 2 bpp
still gives only 24.70 dB luma on the worst desktop; 4.5 bpp passes
35 dB on all planes but misses 0.95 SSIM in SDR 4:4:4. At 1080p,
1 bpp gives 21.69 dB / 0.7825 SSIM on the worst desktop, and 3 bpp
still misses 0.95 SSIM in SDR 4:4:4. There is no universal visual cliff.

The following scores at the selected levels retain separate scene
results. Each cell is minimum Y/Cb/Cr PSNR / minimum luma SSIM across
all tested formats, phases, rates and repeats at that size.

| Size | Level | bpp | Desktop | Game |
|---|---|---:|---|---|
| 720p | Floor | 2.5 | 27.01 / 0.8249 | 37.52 / 0.9468 |
| 720p | Recommended | 5 | 39.99 / 0.9672 | 43.57 / 0.9923 |
| 1080p | Floor | 1.5 | 27.05 / 0.8668 | 34.96 / 0.9135 |
| 1080p | Recommended | 3.2 | 38.26 / 0.9658 | 39.39 / 0.9653 |
| 2160p | Floor | 1.5 | 37.30 / 0.9628 | 35.16 / 0.9188 |
| 2160p | Recommended | 3.2 | 48.26 / 0.9952 | 40.68 / 0.9747 |

Rates at 60 fps, calculated from those measured budget levels and rounded
up to whole Mbps (the functions round up to kbps):

| Stream | Floor | Recommended |
|---|---:|---:|
| 720p60 | 139 Mbps | 277 Mbps |
| 1080p60 | 187 Mbps | 399 Mbps |
| 4K60 | 747 Mbps | 1593 Mbps |

Within each height band, rates scale with width, height and frame rate. A
desktop with smaller text at 1080p or 4K can need more. The recommendation
meets the stated criteria for these scenes; it does not make the earlier stress
chart, every game, or HDR rendering on a real display clean.

Packet overhead, padding and recovery data require network headroom. Ranges
cover every tested scene and format:

| Stream | Requested Mbps | RTP bytes/frame converted to Mbps |
|---|---:|---:|
| 720p60 | 276.480 | 227.8–331.8 |
| 1080p60 | 398.132 | 412.3–458.9 |
| 2160p60 | 1592.525 | 1643.6–1884.9 |

These rates exclude UDP/IP and link headers. The 4K60 recommendation
needs more than gigabit Ethernet. The scaled 4K120 recommendation
(3185 Mbps rounded up) exceeds the current 2 Gbps stream-setup limit;
the harness can supply that budget directly, but a normal stream cannot
negotiate it. The `/bitrate` limit of 500 Mbps can also prevent reaching
the 4K60 target through a runtime change. Use HEVC or AV1 when the client
or network cannot carry the rate. Neither limit is changed here.

Checks passed: `cargo fmt --all`; Clippy for core, Windows and host with
all targets and warnings denied; 171 core tests; 63 host binary tests
(two existing tests ignored); and the PSNR/SSIM unit test. `npm ci` and
`npm run check` passed with 0 errors and 0 warnings across 163 files.
Playwright checked the Overview card at 1440x1080 and 390x844 with mocked
sessions: red below the floor, amber at the floor and below the
recommendation, no warning at the recommendation, no PyroWave warning
for HEVC, and the 720p/4K examples. The cards wrapped without horizontal
overflow; there were no page errors, console warnings/errors or framework
overlays. The temporary web server did not connect to the installed host.

## October 7: 4:4:4 from AMF

Whether the native AMF encoder could offer HDR 4:4:4 HEVC or AV1 was
checked on the RX 7900 XT, driver 32.0.31041.1004, AMF runtime 1.5.2.0, with
a standalone probe outside Rubylight. It cannot; the hardware encodes 4:2:0
only.

- The encoder caps report `HevcMaxProfile` 2 (Main10) and `Av1MaxProfile` 1
  (Main). Input formats are YUV420P, YV12, BGRA, RGBA, ARGB, NV12 and P010;
  output formats NV12 and P010. The runtime's own property table lists
  `HevcProfile` as {Main, Main10} and `Av1Profile` as {Main}, and has no
  chroma-format property. H.264 stops at High.
- `Init` with AYUV, Y410 or Y416 input returns `AMF_INVALID_FORMAT` for both
  codecs. Setting `HevcProfile` to 3, 4, 5 or 16, or `Av1Profile` to 2 or
  higher, returns `AMF_INVALID_ARG` and the profile stays Main10 or Main.
- RGBA, BGRA and R10G10B10A2 input is accepted, and FFprobe reads the output
  as `yuv420p` or `yuv420p10le`: the encoder converts RGB to 4:2:0 itself.
- The driver's D3D12 video encoding API, queried separately on the same GPU,
  supports HEVC Main and Main10 only (not Main12, the 4:2:2 profiles or any of
  the four 4:4:4 profiles) and AV1 Main only (not High or Professional), with
  NV12 and P010 input.
- The AMF SDK on GitHub, 1.5.3 of September 29, has the same profile enums.
  Its 4:4:4 note (1.5.0) is about the video converter, not the encoders.
  AMD's AMF maintainer answered requests for it with "4:4:4 and 4:2:2 codecs
  are currently not supported by hardware" (AMF issue 483, September 2025)
  and said RX 9000 encodes HEVC "4:2:0, 8 and 10 bit" (issue 539). The AMF
  wiki's hardware table says "All codecs are 4:2:0" through VCN 5.0. RDNA 4
  could not be tested here.

Packing 4:4:4 into a larger 4:2:0 picture, or into two 4:2:0 streams as RDP's
AVC444 does, needs a client that reassembles the planes. Moonlight decodes a
single stream at the negotiated size and shows it as is, so a stock client
would display the packed layout. The native AMF encoder keeps refusing 4:4:4,
which the startup probe already treats as unavailable, so an AMD host never
offers it; Moonlight PC then warns "Your host PC doesn't support YUV 4:4:4
streaming" and uses 4:2:0. PyroWave remains the full-chroma path on AMD,
including 10-bit HDR 4:4:4.

## October 7: where WGC loses a millisecond

On the RX 7900 XT, an 8×1-pixel window in the display's bottom-right corner
encoded a changing picture ID. `windows/examples/capture_phase_probe.rs`
matched captured IDs to QPC samples immediately before the renderer's
`Present(1)` call, recorded host acquisition, and waited for a D3D11 event
query after the host-owned texture copy. Only then did it read the eight
pixels. Thus Present-to-copied includes composition, capture delivery and
the completed host copy, but excludes encoder claim, conversion, encoding,
networking and decoding. It is not a stream or input-to-display measurement.

The main batch used a 1920×1080 virtual display reporting 120 Hz, kept alive
by a test-owned SYSTEM task. The probes themselves ran as the signed-in
user. `helper` forces the production WGC helper process and its three shared
textures; `wgc` captures in-process; `ddx` uses Desktop Duplication. Each
condition ran WGC, helper, DDX, DDX, helper, WGC, nine seconds per run,
excluding the first second and final 250 ms. The loaded runs used
`gpu_load SECONDS 1000 0 200`. The fixture checked the installed host log for
a connected client before runs; the JSON does not record other agents' GPU
activity. The range below retains that batch's spread.

Arithmetic means of two per-run means, with their range in parentheses:

| Present to completed host copy, ms | DDX | WGC in-process | WGC helper |
| --- | ---: | ---: | ---: |
| Idle, 59.3 Hz source | 5.05 (5.03–5.08) | 5.88 (5.62–6.15) | 6.05 (5.95–6.16) |
| Idle, vsync-paced source | 6.38 (6.31–6.46) | 6.78 (6.70–6.86) | 7.08 (7.02–7.13) |
| Heavy GPU load, 59.3 Hz requested | 35.62 (35.36–35.89) | 35.23 (34.92–35.55) | 34.28 (31.77–36.79) |

The second row requested 500 Hz and used vsync. Although the virtual display
reported 120 Hz, the recorded Present intervals were about 4.167 ms
(240 calls/s), not 8.333 ms. It should not be described as a verified
120 Hz source. The slow idle source measured 16.863 ms between Presents.
Idle picture coverage was 100%, except one fast DDX run at 99.57%. Under
load it was 86.1–88.7% for DDX, 95.6–96.5% for direct WGC and 89.8–92.6%
for the helper; the table measures pictures actually captured.

Subtracting these batch means, the helper was 1.00 ms behind DDX at 59.3 Hz
and 0.69 ms behind it in the vsync-paced case. The helper-minus-direct WGC
differences were only 0.17 and 0.29 ms. These alternating-run differences
estimate the broker hop, rather than measuring the hop on the same frame.
Most of the idle gap therefore lies in WGC delivery itself. Loaded per-run
means were roughly 32–37 ms; the helper alone varied by 5.02 ms, so there is
no established latency saving from bypassing it under this load.

The earlier SYSTEM-host batch corroborated the idle gap: at 59.3 Hz,
helper/DDX means were 5.85/4.89 ms, and in the vsync-paced case 7.43/6.38 ms.
Its loaded batch ended with a helper first-frame timeout, so it is not a
complete loaded comparison. On the physical display, the initial 59.3 Hz
batch recorded 6.35 ms for direct WGC, 6.21/6.61 ms for the helper and
4.76/4.70 ms for DDX. The final direct-WGC run captured only 36.1% of IDs;
do not pool it into that comparison. Capturing two paths simultaneously
also changed their timings, so the side-by-side runs are excluded above.

The intermediate `stamp_qpc` in these files is the host's legacy capture
stamp: DDX's last presentation/cursor update or WGC's **clamped**
`SystemRelativeTime`. It is not an independent DWM-present observation on
WGC. The separate `wgc_stamp_probe` found 92% of physical-display stamps
in the future at arrival, with mean arrival-minus-stamp about -0.79 ms.
That explains why `detect_mean_ms` could say about 0.12 ms while the
picture-ID probe measured a real delivery gap. New signed
`wgc_stamp_to_host_*` fields and valid/future stamp counts expose this
limitation; the legacy age fields and pacing clock remain unchanged.
[Metric definitions](../docs/performance.md#what-the-numbers-mean) distinguish
these offsets from actual Present-to-copied measurements.

An unbuilt direct IddCx frame ring is estimated to save about 1 ms at idle
if it avoids this WGC delivery gap, and nothing measurable under the tested
load. Only about 0.2–0.3 ms is the helper hop. It is not worth building for
latency now. This is an estimate, not a measurement of an IddCx ring.
The [rc.17 end-to-end fixture](#october-7-rc17-against-rc2-on-the-october-4-fixture)
also puts the component result in context: DDX and WGC picture age were
only 0.15 ms apart idle (14.71 versus 14.86 ms), while WGC was about 2 ms
better under load (33.42 versus 35.36 ms). Component differences do not
predict that entire journey.

Raw artifacts are under
`%USERPROFILE%\AppData\Local\Temp\claude\C--src-butterpollo\7e5651cf-4ad6-4e53-8a70-43a4eede758f\scratchpad\`:
`vphase/u-01` through `u-18` supply the main table, `vphase/v-*` the earlier
SYSTEM batch, and `phase/idle59-*`, `phase/t-*` and `phase/stampsrc-*` the
physical checks. The raw stamp probe's aggregate above was reported by the
original study; those JSON files do not retain its raw stamp samples. Reproduce
with the usage headers in `capture_phase_probe.rs`, `wgc_stamp_probe.rs`
and `session_command_long.rs`. Virtual runs require a test-owned SYSTEM
scheduled task (the `bench-rc17` harness), no active stream, and task/display
cleanup afterward. No capture wait or pool-size optimization was made here.

## October 7: AMF split-frame encoding

AMF's `HevcMultiHwInstanceEncode` and `Av1MultiHwInstanceEncode` let the
driver split one frame across a GPU's video encode engines (AMD's samples
call it "split frame encode", default on); H.264 has no such property. The
original C++ host asked for it before Init when the caps reported more than
one engine and the driver had it off. The Rust host never wrote it. A real
split would show as roughly half the time per frame at large sizes;
alternating whole frames between engines would show as more frames per
second at the same time per frame.

Method: `examples/performance.rs --synthetic 16 --paced --seconds 3` on the
RX 7900 XT, driver and AMF runtime 32.0.31041.1004, with the defaults
(ultra-low latency, `speed`, `vbr_latency`, input converted on the D3D12
compute queue) at 20, 40, 80, 80 and 150 Mbps for 1920×1080, 2560×1440,
3840×2160, 5120×1440 and 7680×2160. Each case ran with the property
untouched (a measurement build that skipped the write), forced on and forced
off, twice, in the order untouched, on, off, off, on, untouched. The
settings line confirmed the value after Init. Before every run the host log
was checked for a stream; a first batch that overlapped a client's stream was
discarded. `--slices N` and `--intra-refresh` were added to the probe for the
interaction runs. Every run's JSON, settings line and decision line are in
`%USERPROFILE%\.codex\artifacts\butterpollo-amf-split-frame-20261007`
(`probe.jsonl` is the discarded batch).

The driver reports two engines for both codecs (`HevcNumOfHwInstances=2`,
`Av1CapNumOfHwInstances=2`) and has the property on before Init with nothing
written, in all 630 runs, with every usage tried. On, off and untouched gave
the same time per frame, the same frames per second and the same bytes per
frame (within 0.2%, the difference of a frame in a three-second run) in all
60 combinations of HEVC and AV1, the five sizes, 60, 120 and 240 fps, SDR
and HDR. At 60 fps, SDR, mean of each run in ms:

| Submission to output | Untouched | Forced on | Forced off |
|---|---|---|---|
| HEVC 1920×1080 | 2.21, 2.20 | 2.29, 2.29 | 2.25, 2.30 |
| HEVC 2560×1440 | 3.29, 3.27 | 3.35, 3.27 | 3.31, 3.34 |
| HEVC 3840×2160 | 6.19, 6.10 | 6.13, 6.13 | 6.18, 6.15 |
| HEVC 5120×1440 | 5.59, 5.65 | 5.68, 5.60 | 5.69, 5.64 |
| HEVC 7680×2160 | 11.34, 11.38 | 11.42, 11.44 | 11.38, 11.24 |
| AV1 1920×1080 | 1.79, 2.02 | 1.86, 1.92 | 1.85, 1.94 |
| AV1 2560×1440 | 3.02, 2.99 | 2.98, 2.91 | 2.96, 3.00 |
| AV1 3840×2160 | 7.10¹, 5.08 | 5.04, 5.08 | 5.08, 5.04 |
| AV1 5120×1440 | 4.88, 4.77 | 4.76, failed¹ | 4.74, 4.79 |
| AV1 7680×2160 | 9.34, 9.41 | 9.44, 9.38 | 9.35, 9.47 |

¹ A stall; see below. HDR and the 120 and 240 fps runs agree the same way:
no median differs by more than 0.2 ms between the three, in no consistent
direction.

The time per frame grows with the picture as one engine's would: 11.3 ms
at 7680×2160 against 6.1 ms at 3840×2160 for HEVC. Where one engine cannot
keep up, a split would have helped most, and nothing changed. Frames per
second without pacing (`--seconds 4`), two runs each:

| Unpaced | Untouched | Forced on | Forced off |
|---|---|---|---|
| HEVC 1920×1080 | 660, 660 | 659, 660 | 660, 660 |
| HEVC 3840×2160 | 196, 196 | 195, 196 | 196, 196 |
| HEVC 5120×1440 | 216, 216 | 216, 216 | 216, 216 |
| HEVC 7680×2160 | 100, 100 | 100, 100 | 100, 100 |
| AV1 1920×1080 | 835, 835 | 836, 835 | 835, 836 |
| AV1 3840×2160 | 248, 248 | 248, 248 | 248, 249 |
| AV1 5120×1440 | 270, 270 | 270, 270 | 270, 269 |
| AV1 7680×2160 | 126, 126 | 126, 126 | 126, 126 |

HEVC at 7680×2160 and 120 fps settles at 100 fps with 82 ms from submission
to output in all three. Frames do overlap in the encoder (196 frames per
second at 3840×2160 against 164 from the time per frame), but a second
engine would nearly double that. So on this GPU and driver one stream uses
one engine, whatever the property says, and the rc.19 note that two frames
in the encoder keep both of a Radeon's instances busy was wrong; its comment
now says so.

Nothing else made the driver split. At 3840×2160 and 60 fps, mean of two
runs each in ms, untouched / on / off:

| Also set | HEVC | AV1 |
|---|---|---|
| 2 slices (AV1: tiles) | 6.30 / 6.33 / 6.34 | 5.22 / 5.23 / 5.23 |
| 4 slices (AV1: tiles) | 6.34 / 6.28 / 6.23 | 5.17 / 5.19 / 5.19 |
| Intra refresh | 6.29 / 6.22 / 6.21 | 5.16 / 5.16 / 5.14 |
| 2 LTR frames | 6.41 / 6.35 / 6.34 | 5.06 / 9.02¹ / 5.01 |
| Quality preset balanced | 6.46 / 6.50 / 6.45 | 5.11 / 5.05 / 5.01 |
| Quality preset quality | 7.47 / 7.46 / 7.48 | 17.48 / 17.43 / 17.44 |
| Pre-analysis | 25.85 / 26.01 / 26.02 | 25.22 / 25.08 / 25.08 |
| CBR | 6.39 / 6.40 / 6.33 | 5.26 / 5.21 / 5.20 |
| Usage transcoding | 25.11 / 24.80 / 25.14 | 12.37 / 12.33 / 12.28 |
| Usage low latency | 23.81 / 25.06 / 25.52 | 12.24 / 12.29 / 12.34 |
| Usage high quality | 20.32 / 20.32 / 20.42 | 19.48 / 19.52 / 19.56 |
| SmartAccess Video on | 6.48 / 6.45 / 6.41 | 5.19 / 5.23 / 5.15 |
| D3D11 input (`gpu_compute_conversion=false`) | 6.45 / 6.42 / 6.55 | 5.35 / 5.23 / 5.24 |

The usages other than ultra-low latency queue frames (p95 of 19-70 ms), so
their means vary more. AV1 needs two tiles above 4096 pixels wide and the driver chose them by
itself at 5120 and 7680 wide; two or four slices or tiles, SmartAccess
Video, the transcoding usage and D3D11 input at 7680×2160 also left the
three alike. Bytes per frame matched in every pair.

Six of these 630 runs stalled once: two gave up (`AMF GPU queue failed to
drain`), one stopped submitting for 35 seconds, and three lost up to 0.55 s
of frames or raised the mean by 2-8 ms while the median stayed put. Four
were forced on, two untouched, none forced off. No stream was running and
the system log has no driver reset, but other builds and an installer ran on
the machine during the matrix. The six cases then ran five more times each
for eight seconds in all three variants (90 runs, twelve minutes of
encoding): no stall, the means within 0.05 ms of each other, and the driver
again had the property on before Init. The stalls do not follow the
property.

`amd_split_frame` now does what the original host did: `auto` (the default)
asks for split-frame encoding only when the caps report more than one engine
and the driver has it off, `enabled` asks whenever there is more than one
engine, and `disabled` turns it off. With one engine, as the RX 9070 XT's HEVC
encoder, nothing is written in any mode, and a driver that rejects the property
keeps the stream. The host logs the setting, the engine count and the driver's
value (`AMF split-frame encoding left to the driver` or `requested`), and the
settings line shows the value after Init. On the RX 7900 XT `auto` writes
nothing.

### October 22: AMD's own sample encoder

Reddit's suggestion was that our split-frame write is wrong, so this repeats
the question without any Rubylight code: AMD's own `VCEEncoderD3D` sample from
the official AMF repository (`GPUOpen-LibrariesAndSDKs/AMF`, master at
`57ee0cfb0e1a`, 2026-10-16), built Release x64 with Build Tools 14.44 and its
default v143 toolset. The sample takes any encoder property by name on the
command line and writes it before Init, which is where the host writes it.
Changes to the sample: the sample's project builds the Vulkan shaders with the
Vulkan SDK, so the build used a copy of the project without those two custom
build steps (nothing in the DX11 path uses them); the ATL component was added to
Build Tools and header-only Vulkan and OpenCL headers were pointed at. In
`RenderEncodePipeline.cpp`, 12 lines after `m_pEncoder->Init()` read back the
multi-instance property and the engine count and print them. Nothing else in
AMD's code was touched.

Method: RX 7900 XT, AMD driver 32.0.32015.7029 (the October 7 matrix ran on
32.0.31041.1004), `-render dx11 -codec hevc|av1 -<Codec>Usage ultralowlatency
-<Codec>QualityPreset speed`, no pacing (the sample submits as fast as the
encoder takes frames), the sample's own rotating-cube render, the usage's
default rate control, 1500 frames at 7680×2160 and 2500 at 3840×2160. Each
case ran untouched, on, off, off, on, untouched with `-HevcMultiHwInstanceEncode
true|false` or `-Av1MultiHwInstanceEncode true|false`. The host service was
checked idle (`RustHostSessionCount` and `RustHostPendingSessionCount` both 0)
before every batch and nothing else was streaming. Frames per second and the
mean time per frame are the sample's own "Frame process time" lines; two runs
per cell, shown as first / second.

| Case | untouched | on | off | read back (untouched / on / off), engines |
| --- | --- | --- | --- | --- |
| HEVC 7680×2160 | 99.0 / 101.3 fps (10.10 / 9.87 ms) | 101.1 / 101.3 (9.89 / 9.87) | 101.1 / 101.3 (9.89 / 9.88) | true / true / false, 2 |
| HEVC 3840×2160 | 197.1 / 197.1 (5.07 / 5.07) | 197.4 / 197.5 (5.07 / 5.06) | 197.3 / 197.6 (5.07 / 5.06) | true / true / false, 2 |
| AV1 7680×2160 | 121.8 / 121.9 (8.21 / 8.20) | 121.9 / 121.8 (8.20 / 8.21) | 122.0 / 121.9 (8.20 / 8.20) | true / true / false, 2 |
| AV1 3840×2160 | 238.4 / 239.2 (4.19 / 4.18) | 238.3 / 238.4 (4.20 / 4.20) | 238.0 / 238.8 (4.20 / 4.19) | true / true / false, 2 |
| HEVC 7680×2160, 2 slices | n/a | 100.9 / 101.3 (9.91 / 9.87) | 101.2 / 101.3 (9.88 / 9.87) | n/a / true / false, 2 |
| AV1 7680×2160, 2 tiles | n/a | 123.1 / 122.0 (8.12 / 8.19) | 121.8 / 121.9 (8.21 / 8.20) | n/a / true / false, 2 |

The driver again had the property on before Init with nothing written, the
readback followed what was written in every run, and both codecs reported two
engines (`HevcNumOfHwInstances=2`, `Av1CapNumOfHwInstances=2`). The slice
count changed the HEVC output (43.5 kB against 43.2 kB per frame); the AV1
`Av1NumTilesPerFrame=2` request produced the same bytes as no request, which
fits the October 7 note that the driver already chooses two tiles at this
width.

Two engines at once: two separate sample processes, HEVC 7680×2160, property
untouched, started together, twice. Each ran 101.4 / 102.0 fps and 101.2 /
101.9 fps, so 203 fps combined against about 101 fps alone. Four processes at
once ran 50.8-51.1 fps each, 203 fps combined again.

On this GPU with AMD's own sample, forcing split-frame encoding on, off or
leaving it alone gave the same frames per second and time per frame (within
1% apart from the first untouched HEVC run, 2% low, no trend) for HEVC and AV1 at 7680×2160 and 3840×2160, with and without
2 slices or tiles, while two separate streams ran side by side at the rate of
one, so the second engine is reachable and one stream did not use it more with
the property on. This matches the October 7 host matrix. Only the sample's
cube content (30-46 kB per frame) was encoded, only the speed preset in
ultra-low latency, and one driver version.

## 2026-10-07: send PyroWave FEC blocks sooner

These measurements use an AMD RX 7900 XT (driver 32.0.31041.1004) and Ryzen 7
5800X3D. The baseline is `523aa283794690a3cd3beb3ca3ed2b172ecd87bb`.

The sender now prepares and sends one FEC block at a time. The coarse block
and its parity leave before later blocks are copied into RTP shards and
encrypted; preparing the next block can use part of the previous batch's
pacing interval. Record packing sizes its fit tree to the records present
and stops updating ancestors once their earliest record is unchanged.
`DetailFec` uses a pre-sized hash map for record lookup; it never depends on
map iteration order. Its stability thresholds, wire budget and FEC plans
are unchanged. An abandoned iterator advances the frame number once a
block has escaped and keeps the consumed sequence numbers and nonces.

Temporary probes timed mapped-bitstream access and SDK packetizing,
`record_frame`, `DetailFec::observe`, layout/FEC planning, shard allocation
and copying, parity, encryption, pacing waits and Winsock calls. They also
recorded the first send batch and final send relative to capture claim and
presentation. These logs are removed from production; the existing
`stream timings` counters remain. "Sent" here means the socket call
returned, not a hardware timestamp on a physical link. The first-batch
measurement includes sending every datagram in that batch.

The encoder probe used `windows/examples/performance.rs --codec pyrowave
--records --hdr --yuv444 --paced --synthetic 8 --seconds 6`, with
`--width 1920 --height 1080 --fps 120 --bitrate 400000` and
`--width 3840 --height 2160 --fps 60 --bitrate 800000`. All eight probes in
the two complete A/B pairs reached the requested cadence. Mean encoder
submission-to-output time, including packetizing and record packing, was
0.969–0.972 ms before / 0.963–0.969 ms after at 1080p, and 2.467–2.565 ms
before / 2.612–2.850 ms after at 4K. These are not GPU-only encode times.

The 12-second encrypted host runs used the isolated `release/e2e.py` fixtures
and the pinned Nonary transport referenced above, with GPU decode removed from
a temporary receiver. The receiver still decrypts and assembles the stream; its
inherited "decoded" counter counts delivered frames in this mode. The source
was a 5120×1440 SDR desktop with the motion strip, scaled to the requested HDR
4:4:4 output. `pacing_max_bitrate_kbps=0` was unchanged: loopback has no
reported physical link speed, so the measured pacing rates were about 442 Mbps
and 880 Mbps, including the existing headroom/traffic-demand rule. The
requested codec rates were 400 and 800 Mbps; negotiation reported 398,988 and
798,988 kbps.

The installed service log ended in `CLIENT DISCONNECTED` before every
GPU batch. Other worktrees were compiling and running GPU tests. Runs
alternated A/B/A/B; a port collision interrupted an intermediate pair,
which is excluded. The repeat used unused fixture ports 51618–51644.
Complete transport runs are `target/pw-pipeline/transport-{before,after}-{1,3}`;
per-stage summaries are in `target/pw-pipeline/transport-summary.json`.
These local artifacts are not shipped.

Mean time per frame in the first complete pair, milliseconds (before → after):

| Stage | 1080p120 / 400 Mbps | 4K60 / 800 Mbps |
|---|---:|---:|
| Mapped access and SDK packetizing | 0.554 → 0.538 | 1.168 → 1.162 |
| Record packing | 0.239 → 0.231 | 0.968 → 0.877 |
| Adaptive detail FEC observation | 0.299 → 0.212 | 0.929 → 0.124 |
| Layout and FEC planning | 0.035 → 0.042 | 0.174 → 0.266 |
| RTP shard allocation/copying | 0.037 → 0.046 | 0.128 → 0.102 |
| Parity computation | 0.001 → 0.002 | 0.007 → 0.007 |
| Headers and encryption | 0.296 → 0.299 | 0.871 → 0.804 |
| Pacing waits | 7.439 → 7.598 | 12.120 → 11.438 |
| Socket sends | 1.264 → 1.455 | 3.393 → 2.744 |

Stage means use each stage's completed frames; frames replaced while
waiting for the sender mean these columns are not one additive timeline.
The large 4K detail-observation change also reflects more frames arriving
within the nominal interval, where the existing controller skips hashing.
It is not a measurement of the map replacement alone. In the second pair,
4K record packing was 1.198 → 1.345 ms and socket sends 4.281 → 4.705 ms,
showing the background-load variation.

Ranges of the two run means, milliseconds:

| Measurement | 1080p before | 1080p after | 4K before | 4K after |
|---|---:|---:|---:|---:|
| Encoder return → first send batch | 5.18–5.23 | 4.35–4.47 | 9.96–10.77 | 2.06–8.84 |
| Encoder return → last send | 13.60–13.61 | 13.38–13.71 | 25.33–26.99 | 17.03–26.05 |
| Present → first send batch | 6.50–8.07 | 5.68–5.80 | 13.02–13.97 | 6.93–14.23 |
| Present → last send | 14.87–16.49 | 14.72–15.05 | 28.39–30.20 | 21.90–31.44 |

Present-to-last-send p95 ranged from 18.77–23.23 ms before to
18.71–19.10 ms after at 1080p, and 36.42–37.96 ms before to
30.91–41.09 ms after at 4K. The stream's own rolling `stream timings`
reported mean present-to-send of 14.85–19.41 → 14.60–14.85 ms and
28.49–30.56 → 20.48–31.34 ms respectively. This supports earlier first
sends after encoder return, but not a consistent 4K present-latency or
1080p completion improvement under this shared load.

A CPU replay separates preparation from that background GPU/capture load.
It used identical synthetic, structurally valid records (not encoded
pictures), 419,536 / 1,679,240 raw bytes,
and verified both packers produced identical framed bytes. It alternated
the old/new packer and controller and the eager/incremental packet APIs
150 times per size after ten warmups, with encrypted 1360-byte packetizer
settings and 20% critical FEC. Detail observation used unchanged frames
20 ms apart. This measures CPU work, not socket or presentation latency:

| Replay stage, mean / p95 ms | 1080p before | 1080p after | 4K before | 4K after |
|---|---:|---:|---:|---:|
| Record packing | 0.403 / 0.542 | 0.307 / 0.405 | 1.177 / 1.481 | 1.067 / 1.380 |
| Detail observation | 0.275 / 0.364 | 0.200 / 0.268 | 1.164 / 1.418 | 0.614 / 0.822 |
| First block ready, including planning | 0.489 / 0.583 | 0.064 / 0.099 | 1.749 / 2.051 | 0.214 / 0.279 |

The eager API must finish every block before returning the first. Building
and releasing all blocks still took 0.531 → 0.467 ms and 1.828 → 1.791 ms;
most of the first-block saving moves later work out of its way. The replay
and its log remain in `target/pw-pipeline/pyrowave_pipeline_probe.rs` and
`target/pw-pipeline/cpu-replay.log`.

Pacing remains the largest cost. The existing sender already uses
`WSASendMsg` scatter/gather with UDP segmentation and `Timer::until_precise`;
there is no additional payload concatenation in the socket path. Sending
at FEC boundaries increased mean socket calls from about 7 to 9 at 1080p
and 21 to 22 at 4K. Critical parity cost only microseconds in these runs,
so no parity worker or timer change was added. The SDK still packetizes
into its reusable CPU buffer before record packing; skipping that copy
would require replacing SDK packetization, not simply borrowing its raw
mapped buffer. Encryption remains in place in the final shards.

Compatibility checks preserve the ordering required by
[Nonary's pinned FEC queue](https://github.com/Nonary/moonlight-common-c/blob/d6a11bc685b41037b352a96f29d08276fe5359ba/src/RtpVideoQueue.c)
and the critical-packet count and restart markers used by its
[depacketizer](https://github.com/Nonary/moonlight-common-c/blob/d6a11bc685b41037b352a96f29d08276fe5359ba/src/VideoDepacketizer.c).
Twelve golden packet fingerprints were regenerated from the baseline
source and match, including framing, critical/detail parity, encryption,
unaligned payload fallback, minimum parity and sequence/frame wraps over
two consecutive frames. Separate tests cover abandoning a partial frame
and the old controller's exact decisions through record reordering,
sequence changes, motion, a nominal-cadence reset and a pause.

The final release build generated all 24 native record/container transport
fixtures. `tests/pyrowave_transport.py` matched parity against the original C++
nanors DLL, recovered 12 deliberately lost coarse shards across the protected
fixtures, and decoded every recovered frame with the independent vendor
decoder. The final encrypted live decoder runs also passed the receiver's
interoperability checks: 1,180/1,180 frames at 1080p and 537/537 at 4K, with
zero partial frames or decoding failures. Earlier shared-GPU decoder runs
logged queue overflows and failures in both versions; the latency comparison
therefore uses the transport-only receiver described above.

The generic e2e evaluator still returns failure because this receiver
does not emit its picture/motion/tone timing fields. These are transport
and PyroWave decode checks, not a strict e2e evaluator pass. Final checks:
`cargo fmt --all`; clippy for core, host and Windows with all targets and
warnings denied; 158 core tests passed; 46 host tests passed with two
pre-existing ignored tests; and the release host/examples build passed.
No web files changed. The installed service and display configuration
were not changed.

## October 7: input on the control thread

Every client's input passes through one control thread: ENet receives the
datagram, the host decrypts and decodes it, and the injector hands it to
Windows or the virtual gamepad driver. Five things on that thread made
input wait; each is now changed and measured against the code before it
(`72b8b0cc`).

Ryzen 7 5800X3D (16 threads), Windows 11 26200, release builds of
`rust/windows/examples/input_path_probe.rs` and
`rust/host/examples/enet_input_probe.rs`, with the stream's process setup
(HIGH priority class, 1 ms timer resolution, power throttling off). Load is
a child process of normal priority class spinning one thread per logical
CPU at the named thread priority, standing in for a CPU-bound game. Mouse
input is always a zero-distance relative move. The pad sections plug one
neutral VHF pad and remove it, and ran while no client streamed. Network
rows are loopback: they include the host's socket calls and scheduling,
but no NIC, interrupt moderation or wire.

### Virtual gamepad calls

Each pad call is a blocking DeviceIoControl into the VHF driver, which
runs in WUDFHost at normal priority. The raw calls (`vhf`, 1,000 states per
round at 1 kHz, 500 feedback polls):

| Load | State p50 | State max | Feedback poll max |
|---|---|---|---|
| None | 136 µs | 0.25, 0.28 ms | 0.12 ms |
| Normal | 53-55 µs | 6.8, 21.9 ms | 11.1 ms |
| Highest | 54-56 µs | 279, 229 ms | 0.34 ms |

They ran on the control thread: every controller state, the feedback poll
every 8 ms and the driver's first open (2.9-6.2 ms) and plug. A pad now
has its own thread; the control thread queues its events and collects
feedback. `pad-contention` sends one controller state and one mouse move
per millisecond through the injector for 6 s, with the 8 ms refresh, and
records when the mouse move is done after its pass began:

| Load | Before p50 / p99 / max | After p50 / p99 / max |
|---|---|---|
| None | 230 µs / 1.37 ms / 6.2 ms | 45 µs / 0.39 ms / 1.8 ms |
| Normal | 107 µs / 2.11 ms / 27.6 ms | 41 µs / 0.12 ms / 2.1 ms |
| Highest | 92 µs / 2.12 ms / 287 ms | 39 µs / 0.13 ms / 2.0 ms |

Queueing a state takes 3-6 µs (max 125 µs); the rest is SendInput itself
(p50 35-39 µs, max 1.8-2.1 ms). A rerun with `--trace` showed the pad
plugged and no driver call failing on its thread. The driver's own
stalls remain and now delay only that pad's input. Raising WUDFHost's
priority would shorten them but changes another process; it is left as
it is. One earlier run of the new code, before the probe timed controller
and mouse separately, recorded a single 344 ms pass with no load; four
later runs without load did not repeat it, and its cause was not found.

### Input thread priority

The control thread registered with MMCSS and then set its own thread
priority, which cancels the boost. `priority` reads the thread's effective
priority (13 without either); `contention` sends loopback datagrams to a
thread waiting as the control loop does, with TIME_CRITICAL spinners on
every CPU, 1,500 each:

| Thread setup | Priority | p50 | p99 | max |
|---|---|---|---|---|
| `Priority::new` (control thread before) | 14 | 3,541 µs | 20,138 µs | 26,062 µs |
| `Priority::input` (control thread now) | 18 | 17 µs | 30 µs | 287 µs |

The run before the change gave 3,535 / 23,274 / 26,983 µs for
`Priority::new`. The gamepad thread uses `Priority::input` too. Capture,
encode, audio and pacing threads still use `Priority::new`; whether they
should keep their boost needs its own measurement beside a game.

### ENet acknowledgement

rusty_enet's `service()` sends a packet's acknowledgement (one sendto,
22-24 µs here) before it returns the packet. The control socket now holds
what ENet sends until the pass has applied its input, then sends it in
order before `host.flush()`. `enet_input_probe`, 3,000 reliable 36-byte
packets on loopback, two alternating runs per mode, median:

| | Acknowledged inside `service()` | Held until after apply |
|---|---|---|
| Client send to server apply step | 69, 71 µs | 46, 46 µs |
| Server wake to first event | 31, 33 µs | 9, 9 µs |


### SendInput per pass

A SendInput call costs about the same for one input as for eight. The
injector now sends a pass's keyboard and mouse input in one call, in
order; touch, pen, and a key or button whose handling depends on an
unsent press of its kind, split the pass. `batch`, zero-distance moves,
600 passes per row, alternating:

| Events per pass | One call each, p50 / p99 | One call per pass, p50 / p99 |
|---|---|---|
| 1 | 37 / 157 µs | 36 / 121 µs |
| 2 | 62 / 339 µs | 33 / 198 µs |
| 4 | 98 / 347 µs | 31 / 212 µs |
| 8 | 180 / 577 µs | 38 / 270 µs |

Mouse moves within a pass were already merged, so this helps a move with
a click or scroll, key combinations and text, which was one call per
character.

### One-off calls

Finding the display for absolute input enumerates every display
(`display_rect`: p50 1.6 ms, max 2.6 ms; `display::monitors` once 21 ms).
It ran when the injector was made with the first input, every 500 ms while
the stream's display did not exist, and on every pass after a rename until
the new name was found. Lookups now run on their own thread; absolute
mouse, touch and pen wait for one still running, other input never does.
The injector is made when the session is first seen.

| `injector` | Before | After |
|---|---|---|
| `Injector::new_options`, existing display, p50 | 3,419 µs | 69 µs |
| `Injector::new_options`, missing display, p50 | 4,069 µs | 68 µs |
| `set_output` + `refresh` per 1 ms pass, display missing, 3 s, max | 18,333 µs | 65 µs |
| Passes over 200 µs | 5 | 0 |

Synthetic touch and pen devices (2.0-3.0 ms to create) are still made on
the first touch or pen event. While a synthetic touch device exists,
Windows reports touch to every application: `SM_MAXIMUMTOUCHES` went from
0 to 32 and `SM_DIGITIZER` from 0 to 193 (integrated multi-touch, ready),
and back after it was destroyed. Making one ahead of time would change
what a game sees in sessions that never touch.

## October 7, 2026: AMF rate control and recovery keyframes

The rc.19 loss report motivated this test: a recovery keyframe can be several
ordinary frames of data, making a congested wireless route worse. These are
local encoder measurements on an AMD Radeon RX 7900 XT, driver 32.0.31041.1004,
AMF runtime 1.5.2.0, Rust 1.98.1 on Windows GNU. RDNA4 was not available.

### Method

`rust/windows/examples/performance.rs` now reports non-keyframe size mean,
p50, p99 and maximum, startup keyframe bytes, each requested recovery
keyframe's bytes, and maximum recovery size divided by mean non-keyframe
size. AV1 keyframes occupy the same `idr_bytes` field as HEVC IDRs.
Percentiles use the sorted sample at `floor((N-1)*p)`. Encode time here is
submission through observed completed output, including GPU conversion and
output polling, not just time inside the codec. The probe drains outstanding
output before reporting, so its submission and completion counts can be
compared. No network traffic is generated.

The main matrix uses the release build, 16 deterministic moving synthetic
pictures, SDR 8-bit 4:2:0, GPU input, two seconds of paced warmup, and eight
seconds of measurement. It requests a recovery keyframe every nominal
second. Startup is measured separately, including the first keyframe before
warmup. Ordinary frames following a recovery request remain in the
non-keyframe distribution: recovering its bit budget is part of rate-control
behaviour. Runs normally complete 481 frames at 60 fps or 961 at 120 fps;
the last scheduled submission is drained after the eight-second interval.

Each mode runs the settings in order and then in reverse order, keeping
comparisons close and exposing run-to-run spread. Before and after every
run, the installed service log is checked for `CLIENT CONNECTED` without a
later `CLIENT DISCONNECTED`; other running performance probes are recorded.
The installed service, driver, display settings and other processes are not
changed. The local JSON results, settings logs and run manifests are in
`target/qa/ratecontrol/`; the tables below retain the per-run measurements.

From the worktree root, after sourcing the supplied `rust-env.ps1`:

```powershell
cargo build --release -p butterpollo-windows --example performance --target-dir target\qa
Copy-Item "$env:BUTTERPOLLO_PYROWAVE_ROOT\bin\libpyrowave-shared-0.dll" target\qa\release\examples\
New-Item -ItemType Directory -Force target\qa\ratecontrol | Out-Null
Set-Content target\qa\ratecontrol\cap2.conf 'amd_max_frame_size=2'
$env:RUST_LOG = 'info'
target\qa\release\examples\performance.exe --encoder amf --synthetic 16 --paced --seconds 8 --width 3840 --height 2160 --fps 60 --bitrate 40000 --codec hevc --idr-interval 60 --config target\qa\ratecontrol\cap2.conf
```

The defaults are ultra-low-latency usage, speed, latency-constrained VBR,
VBAQ on, pre-analysis off, LTR off, HRD off and no filler or frame skipping.
The Rust host sets frame rate and target bitrate at startup. Before this
change it did not explicitly set peak bitrate or VBV there: on this driver
VBR derives a 1.2× peak and one frame of VBV, whereas CBR derives a 1× peak
and **one second** of VBV. The existing bitrate-update path attempts to scale the
driver's positive rate-control values; its AV1 frame-cap property name is
now corrected from `Av1MaxAUSize` to `Av1MaxCompressedFrameSize`.

The new optional limits are applied after frame rate and target bitrate and
read back before Init. Explicit rejected requests produce an AMF setting error.
`LowLatencyInternal` and `InputQueueSize` remain untouched by default.

| Table setting | Configuration added to the defaults |
|---|---|
| default | none |
| cbr | `amd_rc=cbr` |
| cbr_vbv1 | `amd_rc=cbr`, `amd_vbv_buffer_frames=1` |
| peak1 / peak15 / peak2 | `amd_peak_bitrate_ratio=1` / `1.5` / `2` |
| vbv05 / vbv1 / vbv2 | `amd_vbv_buffer_frames=0.5` / `1` / `2` |
| cap1 / cap2 / cap4 | `amd_max_frame_size=1` / `2` / `4` |
| hrd | `amd_enforce_hrd=true` |
| cap2hrd | `amd_max_frame_size=2`, `amd_enforce_hrd=true` |
| intra | `--intra-refresh --idr-interval 0`; continuous refresh, no forced recovery keyframes |
| intra_idr | `--intra-refresh` with the same once-per-second recovery requests |

Frame budgets are bitrate divided by the exact negotiated frame rate. AMF
expects bits for VBV, `MaxAUSize`, `HevcMaxAUSize` and
`Av1MaxCompressedFrameSize`; the tables report bytes. The names and units
match the retained SDK and AMD's [HEVC header](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/public/include/components/VideoEncoderHEVC.h)
and [AV1 header](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/public/include/components/VideoEncoderAV1.h).

### Screening: 4K60 at 40 Mbps

The initial screening used a development build with 20 paced warmup frames
and eight seconds of measurement. Builds overlapped some of these runs, so
the release matrix is the basis for timing comparisons. Its byte-size
comparisons are repeatable; each second pass reverses the settings order.

| Mode / Mbps | Setting / run | Non-keyframes | Steady mean / p50 / p99 / max (bytes) | Startup bytes / steady mean | Recovery max bytes / steady mean | Encode mean / p99 (ms) |
|---|---|---:|---|---:|---|---|
| HEVC 3840×2160@60 / 40 | default / 1 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.117 / 6.817 |
| HEVC 3840×2160@60 / 40 | cbr / 1 | 473 | 79849 / 77566 / 143632 / 158407 | 615045 / 7.70× | 242453 / 3.04× | 5.932 / 6.818 |
| HEVC 3840×2160@60 / 40 | peak1 / 1 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.144 / 6.822 |
| HEVC 3840×2160@60 / 40 | peak15 / 1 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.269 / 6.885 |
| HEVC 3840×2160@60 / 40 | peak2 / 1 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.206 / 6.818 |
| HEVC 3840×2160@60 / 40 | vbv05 / 1 | 473 | 78514 / 78002 / 92474 / 105469 | 213837 / 2.72× | 278811 / 3.55× | 6.233 / 6.842 |
| HEVC 3840×2160@60 / 40 | vbv1 / 1 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.226 / 6.878 |
| HEVC 3840×2160@60 / 40 | vbv2 / 1 | 473 | 80504 / 80229 / 98435 / 105270 | 293486 / 3.65× | 248412 / 3.09× | 6.185 / 6.784 |
| HEVC 3840×2160@60 / 40 | cap1 / 1 | 473 | 79400 / 79008 / 93301 / 94779 | 178313 / 2.25× | 92557 / 1.17× | 6.186 / 6.819 |
| HEVC 3840×2160@60 / 40 | cap2 / 1 | 473 | 79194 / 78595 / 93307 / 93494 | 245474 / 3.10× | 167152 / 2.11× | 6.165 / 6.800 |
| HEVC 3840×2160@60 / 40 | cap4 / 1 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.180 / 6.802 |
| HEVC 3840×2160@60 / 40 | hrd / 1 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.191 / 6.792 |
| HEVC 3840×2160@60 / 40 | cap2hrd / 1 | 473 | 79194 / 78595 / 93307 / 93494 | 245474 / 3.10× | 167152 / 2.11× | 6.185 / 6.858 |
| HEVC 3840×2160@60 / 40 | intra / 1 | 481 | 82031 / 80867 / 93440 / 94347 | 245658 / 2.99× | — | 6.236 / 6.933 |
| HEVC 3840×2160@60 / 40 | intra_idr / 1 | 473 | 79901 / 79312 / 93137 / 96257 | 245658 / 3.07× | 243298 / 3.04× | 6.191 / 6.797 |
| HEVC 3840×2160@60 / 40 | intra_idr / 2 | 473 | 79901 / 79312 / 93137 / 96257 | 245658 / 3.07× | 243298 / 3.04× | 6.170 / 6.758 |
| HEVC 3840×2160@60 / 40 | intra / 2 | 481 | 82031 / 80867 / 93440 / 94347 | 245658 / 2.99× | — | 6.086 / 6.677 |
| HEVC 3840×2160@60 / 40 | cap2hrd / 2 | 473 | 79194 / 78595 / 93307 / 93494 | 245474 / 3.10× | 167152 / 2.11× | 6.122 / 6.712 |
| HEVC 3840×2160@60 / 40 | hrd / 2 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 5.999 / 6.404 |
| HEVC 3840×2160@60 / 40 | cap4 / 2 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.087 / 6.711 |
| HEVC 3840×2160@60 / 40 | cap2 / 2 | 473 | 79194 / 78595 / 93307 / 93494 | 245474 / 3.10× | 167152 / 2.11× | 6.105 / 6.758 |
| HEVC 3840×2160@60 / 40 | cap1 / 2 | 473 | 79400 / 79008 / 93301 / 94779 | 178313 / 2.25× | 92557 / 1.17× | 6.144 / 6.772 |
| HEVC 3840×2160@60 / 40 | vbv2 / 2 | 473 | 80504 / 80229 / 98435 / 105270 | 293486 / 3.65× | 248412 / 3.09× | 6.233 / 6.826 |
| HEVC 3840×2160@60 / 40 | vbv1 / 2 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.205 / 6.819 |
| HEVC 3840×2160@60 / 40 | vbv05 / 2 | 473 | 78514 / 78002 / 92474 / 105469 | 213837 / 2.72× | 278811 / 3.55× | 6.242 / 6.842 |
| HEVC 3840×2160@60 / 40 | peak2 / 2 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.291 / 6.887 |
| HEVC 3840×2160@60 / 40 | peak15 / 2 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.155 / 6.796 |
| HEVC 3840×2160@60 / 40 | peak1 / 2 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.077 / 6.873 |
| HEVC 3840×2160@60 / 40 | cbr / 2 | 473 | 79849 / 77566 / 143632 / 158407 | 615045 / 7.70× | 242453 / 3.04× | 5.988 / 6.704 |
| HEVC 3840×2160@60 / 40 | default / 2 | 473 | 79908 / 79312 / 93172 / 96174 | 245474 / 3.07× | 242986 / 3.04× | 6.009 / 6.587 |
| AV1 3840×2160@60 / 40 | default / 1 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 4.967 / 5.663 |
| AV1 3840×2160@60 / 40 | cbr / 1 | 473 | 82409 / 81892 / 128763 / 141099 | 307290 / 3.73× | 162898 / 1.98× | 5.042 / 5.701 |
| AV1 3840×2160@60 / 40 | peak1 / 1 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 4.919 / 5.576 |
| AV1 3840×2160@60 / 40 | peak15 / 1 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 5.033 / 5.770 |
| AV1 3840×2160@60 / 40 | peak2 / 1 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 4.885 / 5.449 |
| AV1 3840×2160@60 / 40 | vbv05 / 1 | 473 | 78369 / 79091 / 93732 / 99924 | 158745 / 2.03× | 155231 / 1.98× | 4.908 / 5.713 |
| AV1 3840×2160@60 / 40 | vbv1 / 1 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 4.958 / 5.477 |
| AV1 3840×2160@60 / 40 | vbv2 / 1 | 473 | 79098 / 79399 / 105131 / 115271 | 225386 / 2.85× | 160404 / 2.03× | 4.988 / 5.808 |
| AV1 3840×2160@60 / 40 | cap1 / 1 | 473 | 78197 / 79029 / 93730 / 94345 | 132770 / 1.70× | 81211 / 1.04× | 5.040 / 5.714 |
| AV1 3840×2160@60 / 40 | cap2 / 1 | 473 | 78997 / 79251 / 103113 / 111306 | 185400 / 2.35× | 152792 / 1.93× | 4.982 / 5.605 |
| AV1 3840×2160@60 / 40 | cap4 / 1 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 5.058 / 5.787 |
| AV1 3840×2160@60 / 40 | hrd / 1 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 4.953 / 5.754 |
| AV1 3840×2160@60 / 40 | cap2hrd / 1 | 473 | 78997 / 79251 / 103113 / 111306 | 185400 / 2.35× | 152792 / 1.93× | 5.044 / 5.802 |
| AV1 3840×2160@60 / 40 | intra / 1 | 481 | 80105 / 79238 / 93845 / 94157 | 185400 / 2.31× | — | 4.904 / 5.434 |
| AV1 3840×2160@60 / 40 | intra_idr / 1 | 473 | 78811 / 79184 / 105086 / 112245 | 185400 / 2.35× | 153888 / 1.95× | 5.017 / 5.789 |
| AV1 3840×2160@60 / 40 | intra_idr / 2 | 473 | 78811 / 79184 / 105086 / 112245 | 185400 / 2.35× | 153888 / 1.95× | 4.964 / 5.496 |
| AV1 3840×2160@60 / 40 | intra / 2 | 481 | 80105 / 79238 / 93845 / 94157 | 185400 / 2.31× | — | 4.948 / 5.575 |
| AV1 3840×2160@60 / 40 | cap2hrd / 2 | 473 | 78997 / 79251 / 103113 / 111306 | 185400 / 2.35× | 152792 / 1.93× | 5.082 / 5.799 |
| AV1 3840×2160@60 / 40 | hrd / 2 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 5.092 / 5.856 |
| AV1 3840×2160@60 / 40 | cap4 / 2 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 5.113 / 5.856 |
| AV1 3840×2160@60 / 40 | cap2 / 2 | 473 | 78997 / 79251 / 103113 / 111306 | 185400 / 2.35× | 152792 / 1.93× | 4.972 / 5.630 |
| AV1 3840×2160@60 / 40 | cap1 / 2 | 473 | 78197 / 79029 / 93730 / 94345 | 132770 / 1.70× | 81211 / 1.04× | 4.950 / 5.761 |
| AV1 3840×2160@60 / 40 | vbv2 / 2 | 473 | 79098 / 79399 / 105131 / 115271 | 225386 / 2.85× | 160404 / 2.03× | 4.983 / 5.564 |
| AV1 3840×2160@60 / 40 | vbv1 / 2 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 5.024 / 5.867 |
| AV1 3840×2160@60 / 40 | vbv05 / 2 | 473 | 78369 / 79091 / 93732 / 99924 | 158745 / 2.03× | 155231 / 1.98× | 5.104 / 5.804 |
| AV1 3840×2160@60 / 40 | peak2 / 2 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 5.011 / 5.861 |
| AV1 3840×2160@60 / 40 | peak15 / 2 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 4.976 / 5.598 |
| AV1 3840×2160@60 / 40 | peak1 / 2 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 5.203 / 5.928 |
| AV1 3840×2160@60 / 40 | cbr / 2 | 473 | 82409 / 81892 / 128763 / 141099 | 307290 / 3.73× | 162898 / 1.98× | 5.055 / 5.755 |
| AV1 3840×2160@60 / 40 | default / 2 | 473 | 79001 / 79208 / 103293 / 113084 | 185400 / 2.35× | 153531 / 1.94× | 5.087 / 5.899 |

### Release matrix

Rows marked **†** overlap a live service stream (15:50:54–15:51:34 UTC). They are retained as raw observations, excluded from latency comparisons, and repeated below. The AV1 4K40 default run completed 455 frames and the HEVC 4K80 cap1 run 448; all other matrix runs completed the usual 481 or 961. Every submitted frame was returned.

| Mode / Mbps | Setting / run | Non-keyframes | Steady mean / p50 / p99 / max (bytes) | Startup bytes / steady mean | Recovery max bytes / steady mean | Encode mean / p99 (ms) |
|---|---|---:|---|---:|---|---|
| HEVC 1920×1080@60 / 20 | default / 1 | 473 | 40681 / 41542 / 48500 / 49616 | 90690 / 2.23× | 96594 / 2.37× | 2.284 / 2.779 |
| HEVC 1920×1080@60 / 20 | cbr / 1 | 473 | 41054 / 41130 / 52972 / 55554 | 154192 / 3.76× | 80037 / 1.95× | 2.161 / 2.747 |
| HEVC 1920×1080@60 / 20 | cbr_vbv1 / 1 | 473 | 41038 / 41302 / 49118 / 50360 | 154192 / 3.76× | 86468 / 2.11× | 2.149 / 2.728 |
| HEVC 1920×1080@60 / 20 | cap1 / 1 | 473 | 40361 / 40935 / 48345 / 48722 | 77843 / 1.93× | 47530 / 1.18× | 2.250 / 2.888 |
| HEVC 1920×1080@60 / 20 | cap2 / 1 | 473 | 40669 / 41544 / 48338 / 50587 | 90690 / 2.23× | 92231 / 2.27× | 2.217 / 2.854 |
| HEVC 1920×1080@60 / 20 | intra / 1 | 481 | 41309 / 42372 / 48517 / 48668 | 90746 / 2.20× | — | 2.199 / 2.787 |
| HEVC 1920×1080@60 / 20 | intra / 2 | 481 | 41309 / 42372 / 48517 / 48668 | 90746 / 2.20× | — | 2.254 / 3.214 |
| HEVC 1920×1080@60 / 20 | cap2 / 2 | 473 | 40669 / 41544 / 48338 / 50587 | 90690 / 2.23× | 92231 / 2.27× | 2.278 / 2.901 |
| HEVC 1920×1080@60 / 20 | cap1 / 2 | 473 | 40361 / 40935 / 48345 / 48722 | 77843 / 1.93× | 47530 / 1.18× | 2.448 / 3.190 |
| HEVC 1920×1080@60 / 20 | cbr_vbv1 / 2 | 473 | 41038 / 41302 / 49118 / 50360 | 154192 / 3.76× | 86468 / 2.11× | 2.423 / 2.980 |
| HEVC 1920×1080@60 / 20 | cbr / 2 | 473 | 41054 / 41130 / 52972 / 55554 | 154192 / 3.76× | 80037 / 1.95× | 2.410 / 2.852 |
| HEVC 1920×1080@60 / 20 | default / 2 | 473 | 40681 / 41542 / 48500 / 49616 | 90690 / 2.23× | 96594 / 2.37× | 2.391 / 3.216 |
| AV1 1920×1080@60 / 20 | default / 1 | 473 | 41146 / 41475 / 47605 / 56855 | 66499 / 1.62× | 76640 / 1.86× | 1.959 / 2.145 |
| AV1 1920×1080@60 / 20 | cbr / 1 | 473 | 41384 / 40699 / 55727 / 59922 | 79012 / 1.91× | 63611 / 1.54× | 1.851 / 2.036 |
| AV1 1920×1080@60 / 20 | cbr_vbv1 / 1 | 473 | 41141 / 41258 / 49984 / 54971 | 79012 / 1.92× | 64778 / 1.57× | 1.886 / 2.241 |
| AV1 1920×1080@60 / 20 | cap1 / 1 | 473 | 40883 / 41081 / 47339 / 54164 | 56286 / 1.38× | 43347 / 1.06× | 1.822 / 1.996 |
| AV1 1920×1080@60 / 20 | cap2 / 1 | 473 | 41146 / 41475 / 47605 / 56855 | 66499 / 1.62× | 76640 / 1.86× | 1.782 / 1.905 |
| AV1 1920×1080@60 / 20 | intra / 1 | 481 | 41235 / 41608 / 46994 / 47452 | 66499 / 1.61× | — | 1.827 / 2.072 |
| AV1 1920×1080@60 / 20 | intra / 2 | 481 | 41235 / 41608 / 46994 / 47452 | 66499 / 1.61× | — | 1.845 / 2.024 |
| AV1 1920×1080@60 / 20 | cap2 / 2 | 473 | 41146 / 41475 / 47605 / 56855 | 66499 / 1.62× | 76640 / 1.86× | 1.858 / 2.062 |
| AV1 1920×1080@60 / 20 | cap1 / 2 | 473 | 40883 / 41081 / 47339 / 54164 | 56286 / 1.38× | 43347 / 1.06× | 1.745 / 1.954 |
| AV1 1920×1080@60 / 20 | cbr_vbv1 / 2 | 473 | 41141 / 41258 / 49984 / 54971 | 79012 / 1.92× | 64778 / 1.57× | 1.818 / 1.957 |
| AV1 1920×1080@60 / 20 | cbr / 2 | 473 | 41384 / 40699 / 55727 / 59922 | 79012 / 1.91× | 63611 / 1.54× | 1.818 / 2.117 |
| AV1 1920×1080@60 / 20 | default / 2 | 473 | 41146 / 41475 / 47605 / 56855 | 66499 / 1.62× | 76640 / 1.86× | 1.804 / 1.960 |
| HEVC 1920×1080@60 / 40 | default / 1 | 473 | 81591 / 82190 / 95755 / 98074 | 109896 / 1.35× | 165972 / 2.03× | 2.238 / 2.825 |
| HEVC 1920×1080@60 / 40 | cbr / 1 | 473 | 82498 / 81777 / 96972 / 102106 | 154192 / 1.87× | 136004 / 1.65× | 2.283 / 2.888 |
| HEVC 1920×1080@60 / 40 | cbr_vbv1 / 1 | 473 | 81987 / 82608 / 92698 / 93796 | 154192 / 1.88× | 140645 / 1.72× | 2.207 / 2.734 |
| HEVC 1920×1080@60 / 40 | cap1 / 1 | 473 | 81706 / 82426 / 94881 / 95460 | 109896 / 1.35× | 96997 / 1.19× | 2.178 / 2.827 |
| HEVC 1920×1080@60 / 40 | cap2 / 1 | 473 | 81683 / 82237 / 94522 / 97190 | 109896 / 1.35× | 153716 / 1.88× | 2.287 / 2.796 |
| HEVC 1920×1080@60 / 40 | intra / 1 | 481 | 82018 / 82519 / 94952 / 96880 | 109929 / 1.34× | — | 2.228 / 2.791 |
| HEVC 1920×1080@60 / 40 | intra / 2 | 481 | 82018 / 82519 / 94952 / 96880 | 109929 / 1.34× | — | 2.341 / 2.848 |
| HEVC 1920×1080@60 / 40 | cap2 / 2 | 473 | 81683 / 82237 / 94522 / 97190 | 109896 / 1.35× | 153716 / 1.88× | 2.338 / 2.877 |
| HEVC 1920×1080@60 / 40 | cap1 / 2 | 473 | 81706 / 82426 / 94881 / 95460 | 109896 / 1.35× | 96997 / 1.19× | 2.290 / 2.865 |
| HEVC 1920×1080@60 / 40 | cbr_vbv1 / 2 | 473 | 81987 / 82608 / 92698 / 93796 | 154192 / 1.88× | 140645 / 1.72× | 2.357 / 3.008 |
| HEVC 1920×1080@60 / 40 | cbr / 2 | 473 | 82498 / 81777 / 96972 / 102106 | 154192 / 1.87× | 136004 / 1.65× | 2.396 / 3.227 |
| HEVC 1920×1080@60 / 40 | default / 2 | 473 | 81591 / 82190 / 95755 / 98074 | 109896 / 1.35× | 165972 / 2.03× | 2.353 / 3.152 |
| AV1 1920×1080@60 / 40 | default / 1 | 473 | 82772 / 83111 / 100624 / 105872 | 71907 / 0.87× | 120405 / 1.45× | 1.972 / 2.256 |
| AV1 1920×1080@60 / 40 | cbr / 1 | 473 | 83206 / 84552 / 97402 / 97986 | 79012 / 0.95× | 100260 / 1.20× | 1.873 / 2.127 |
| AV1 1920×1080@60 / 40 | cbr_vbv1 / 1 | 473 | 82008 / 84113 / 94902 / 95264 | 79012 / 0.96× | 97148 / 1.18× | 1.843 / 2.080 |
| AV1 1920×1080@60 / 40 | cap1 / 1 | 473 | 82055 / 82708 / 100503 / 101143 | 71907 / 0.88× | 77800 / 0.95× | 1.814 / 2.001 |
| AV1 1920×1080@60 / 40 | cap2 / 1 | 473 | 82772 / 83111 / 100624 / 105872 | 71907 / 0.87× | 120405 / 1.45× | 1.877 / 2.057 |
| AV1 1920×1080@60 / 40 | intra / 1 | 481 | 82726 / 83067 / 100639 / 101241 | 71907 / 0.87× | — | 1.881 / 2.091 |
| AV1 1920×1080@60 / 40 | intra / 2 | 481 | 82726 / 83067 / 100639 / 101241 | 71907 / 0.87× | — | 1.901 / 2.075 |
| AV1 1920×1080@60 / 40 | cap2 / 2 | 473 | 82772 / 83111 / 100624 / 105872 | 71907 / 0.87× | 120405 / 1.45× | 1.894 / 2.107 |
| AV1 1920×1080@60 / 40 | cap1 / 2 | 473 | 82055 / 82708 / 100503 / 101143 | 71907 / 0.88× | 77800 / 0.95× | 1.874 / 2.035 |
| AV1 1920×1080@60 / 40 | cbr_vbv1 / 2 | 473 | 82008 / 84113 / 94902 / 95264 | 79012 / 0.96× | 97148 / 1.18× | 1.861 / 2.102 |
| AV1 1920×1080@60 / 40 | cbr / 2 | 473 | 83206 / 84552 / 97402 / 97986 | 79012 / 0.95× | 100260 / 1.20× | 1.856 / 1.990 |
| AV1 1920×1080@60 / 40 | default / 2 | 473 | 82772 / 83111 / 100624 / 105872 | 71907 / 0.87× | 120405 / 1.45× | 1.877 / 2.060 |
| HEVC 1920×1080@60 / 80 | default / 1 | 473 | 163579 / 162262 / 191592 / 192365 | 127237 / 0.78× | 309871 / 1.89× | 2.344 / 2.741 |
| HEVC 1920×1080@60 / 80 | cbr / 1 | 473 | 165849 / 165127 / 176931 / 190737 | 154192 / 0.93× | 260312 / 1.57× | 2.397 / 2.823 |
| HEVC 1920×1080@60 / 80 | cbr_vbv1 / 1 | 473 | 163872 / 167352 / 184602 / 185931 | 154192 / 0.94× | 286142 / 1.75× | 2.337 / 2.727 |
| HEVC 1920×1080@60 / 80 | cap1 / 1 | 473 | 163736 / 162409 / 191766 / 192129 | 127237 / 0.78× | 187840 / 1.15× | 2.334 / 2.776 |
| HEVC 1920×1080@60 / 80 | cap2 / 1 | 473 | 163279 / 161757 / 191459 / 192365 | 127237 / 0.78× | 309675 / 1.90× | 2.366 / 2.749 |
| HEVC 1920×1080@60 / 80 | intra / 1 | 481 | 164616 / 163113 / 191800 / 192106 | 127343 / 0.77× | — | 2.307 / 2.713 |
| HEVC 1920×1080@60 / 80 | intra / 2 | 481 | 164616 / 163113 / 191800 / 192106 | 127343 / 0.77× | — | 2.287 / 2.672 |
| HEVC 1920×1080@60 / 80 | cap2 / 2 | 473 | 163279 / 161757 / 191459 / 192365 | 127237 / 0.78× | 309675 / 1.90× | 2.286 / 2.700 |
| HEVC 1920×1080@60 / 80 | cap1 / 2 | 473 | 163736 / 162409 / 191766 / 192129 | 127237 / 0.78× | 187840 / 1.15× | 2.264 / 2.678 |
| HEVC 1920×1080@60 / 80 | cbr_vbv1 / 2 | 473 | 163872 / 167352 / 184602 / 185931 | 154192 / 0.94× | 286142 / 1.75× | 2.285 / 2.718 |
| HEVC 1920×1080@60 / 80 | cbr / 2 | 473 | 165849 / 165127 / 176931 / 190737 | 154192 / 0.93× | 260312 / 1.57× | 2.319 / 2.692 |
| HEVC 1920×1080@60 / 80 | default / 2 | 473 | 163579 / 162262 / 191592 / 192365 | 127237 / 0.78× | 309871 / 1.89× | 2.298 / 2.698 |
| AV1 1920×1080@60 / 80 | default / 1 | 473 | 163248 / 162500 / 204673 / 208384 | 74987 / 0.46× | 246750 / 1.51× | 1.962 / 2.204 |
| AV1 1920×1080@60 / 80 | cbr / 1 | 473 | 165879 / 166058 / 200869 / 230081 | 79012 / 0.48× | 238012 / 1.43× | 1.977 / 2.335 |
| AV1 1920×1080@60 / 80 | cbr_vbv1 / 1 | 473 | 161297 / 166396 / 203637 / 204852 | 79012 / 0.49× | 199973 / 1.24× | 1.907 / 1.977 |
| AV1 1920×1080@60 / 80 | cap1 / 1 | 473 | 161583 / 159934 / 205139 / 205607 | 74987 / 0.46× | 137621 / 0.85× | 1.975 / 2.139 |
| AV1 1920×1080@60 / 80 | cap2 / 1 | 473 | 163522 / 163182 / 205090 / 208100 | 74987 / 0.46× | 246750 / 1.51× | 1.916 / 2.031 |
| AV1 1920×1080@60 / 80 | intra / 1 | 481 | 164629 / 163207 / 205290 / 206221 | 74987 / 0.46× | — | 2.000 / 2.343 |
| AV1 1920×1080@60 / 80 | intra / 2 | 481 | 164629 / 163207 / 205290 / 206221 | 74987 / 0.46× | — | 1.930 / 2.058 |
| AV1 1920×1080@60 / 80 | cap2 / 2 | 473 | 163522 / 163182 / 205090 / 208100 | 74987 / 0.46× | 246750 / 1.51× | 1.921 / 2.116 |
| AV1 1920×1080@60 / 80 | cap1 / 2 | 473 | 161583 / 159934 / 205139 / 205607 | 74987 / 0.46× | 137621 / 0.85× | 1.953 / 2.260 |
| AV1 1920×1080@60 / 80 | cbr_vbv1 / 2 | 473 | 161297 / 166396 / 203637 / 204852 | 79012 / 0.49× | 199973 / 1.24× | 1.984 / 2.169 |
| AV1 1920×1080@60 / 80 | cbr / 2 | 473 | 165879 / 166058 / 200869 / 230081 | 79012 / 0.48× | 238012 / 1.43× | 1.910 / 2.076 |
| AV1 1920×1080@60 / 80 | default / 2 | 473 | 163248 / 162500 / 204673 / 208384 | 74987 / 0.46× | 246750 / 1.51× | 1.926 / 2.154 |
| HEVC 2560×1440@120 / 20 | default / 1 | 953 | 20500 / 20082 / 24354 / 24406 | 65915 / 3.22× | 63289 / 3.09× | 3.157 / 3.706 |
| HEVC 2560×1440@120 / 20 | cbr / 1 | 953 | 20846 / 20221 / 26649 / 30504 | 271818 / 13.04× | 51971 / 2.49× | 3.111 / 3.616 |
| HEVC 2560×1440@120 / 20 | cbr_vbv1 / 1 | 953 | 20563 / 20171 / 25463 / 25558 | 154202 / 7.50× | 58660 / 2.85× | 3.118 / 3.668 |
| HEVC 2560×1440@120 / 20 | cap1 / 1 | 953 | 20538 / 20155 / 24346 / 24426 | 45481 / 2.21× | 20680 / 1.01× | 3.177 / 3.703 |
| HEVC 2560×1440@120 / 20 | cap2 / 1 | 953 | 20506 / 20059 / 24370 / 24406 | 65915 / 3.21× | 41048 / 2.00× | 3.111 / 3.636 |
| HEVC 2560×1440@120 / 20 | intra / 1 | 961 | 20665 / 20139 / 24343 / 24402 | 65854 / 3.19× | — | 3.095 / 3.611 |
| HEVC 2560×1440@120 / 20 | intra / 2 | 961 | 20665 / 20139 / 24343 / 24402 | 65854 / 3.19× | — | 3.119 / 3.634 |
| HEVC 2560×1440@120 / 20 | cap2 / 2 | 953 | 20506 / 20059 / 24370 / 24406 | 65915 / 3.21× | 41048 / 2.00× | 3.163 / 3.726 |
| HEVC 2560×1440@120 / 20 | cap1 / 2 | 953 | 20538 / 20155 / 24346 / 24426 | 45481 / 2.21× | 20680 / 1.01× | 3.133 / 3.667 |
| HEVC 2560×1440@120 / 20 | cbr_vbv1 / 2 | 953 | 20563 / 20171 / 25463 / 25558 | 154202 / 7.50× | 58660 / 2.85× | 3.089 / 3.577 |
| HEVC 2560×1440@120 / 20 | cbr / 2 | 953 | 20846 / 20221 / 26649 / 30504 | 271818 / 13.04× | 51971 / 2.49× | 3.136 / 3.676 |
| HEVC 2560×1440@120 / 20 | default / 2 | 953 | 20500 / 20082 / 24354 / 24406 | 65915 / 3.22× | 63289 / 3.09× | 3.158 / 3.696 |
| AV1 2560×1440@120 / 20 | default / 1 | 953 | 20544 / 20317 / 24969 / 25066 | 53028 / 2.58× | 47847 / 2.33× | 2.558 / 3.074 |
| AV1 2560×1440@120 / 20 | cbr / 1 | 953 | 20593 / 19360 / 25502 / 46398 | 134832 / 6.55× | 43867 / 2.13× | 2.547 / 3.026 |
| AV1 2560×1440@120 / 20 | cbr_vbv1 / 1 | 953 | 20823 / 20412 / 27197 / 27797 | 86642 / 4.16× | 49660 / 2.38× | 2.546 / 3.006 |
| AV1 2560×1440@120 / 20 | cap1 / 1 | 953 | 20571 / 20223 / 24976 / 25106 | 34657 / 1.68× | 23931 / 1.16× | 2.605 / 3.143 |
| AV1 2560×1440@120 / 20 | cap2 / 1 | 953 | 20553 / 20252 / 24944 / 25196 | 53028 / 2.58× | 40503 / 1.97× | 2.602 / 3.145 |
| AV1 2560×1440@120 / 20 | intra / 1 | 961 | 20649 / 20271 / 24989 / 25088 | 53028 / 2.57× | — | 2.583 / 3.138 |
| AV1 2560×1440@120 / 20 | intra / 2 | 961 | 20649 / 20271 / 24989 / 25088 | 53028 / 2.57× | — | 2.694 / 3.163 |
| AV1 2560×1440@120 / 20 | cap2 / 2 | 953 | 20553 / 20252 / 24944 / 25196 | 53028 / 2.58× | 40503 / 1.97× | 2.574 / 3.136 |
| AV1 2560×1440@120 / 20 | cap1 / 2 | 953 | 20571 / 20223 / 24976 / 25106 | 34657 / 1.68× | 23931 / 1.16× | 2.558 / 3.089 |
| AV1 2560×1440@120 / 20 | cbr_vbv1 / 2 | 953 | 20823 / 20412 / 27197 / 27797 | 86642 / 4.16× | 49660 / 2.38× | 2.737 / 3.238 |
| AV1 2560×1440@120 / 20 | cbr / 2 | 953 | 20593 / 19360 / 25502 / 46398 | 134832 / 6.55× | 43867 / 2.13× | 2.573 / 3.107 |
| AV1 2560×1440@120 / 20 | default / 2 | 953 | 20544 / 20317 / 24969 / 25066 | 53028 / 2.58× | 47847 / 2.33× | 2.573 / 3.130 |
| HEVC 2560×1440@120 / 40 | default / 1 | 953 | 40275 / 41108 / 46697 / 54280 | 120344 / 2.99× | 137967 / 3.43× | 3.170 / 3.685 |
| HEVC 2560×1440@120 / 40 | cbr / 1 | 953 | 40896 / 40339 / 51911 / 82915 | 273504 / 6.69× | 102765 / 2.51× | 3.193 / 3.693 |
| HEVC 2560×1440@120 / 40 | cbr_vbv1 / 1 | 953 | 40861 / 41148 / 48109 / 58489 | 241930 / 5.92× | 121840 / 2.98× | 3.101 / 3.624 |
| HEVC 2560×1440@120 / 40 | cap1 / 1 | 953 | 40056 / 40840 / 46687 / 47410 | 87706 / 2.19× | 44501 / 1.11× | 3.113 / 3.655 |
| HEVC 2560×1440@120 / 40 | cap2 / 1 | 953 | 40109 / 40920 / 46697 / 47624 | 120344 / 3.00× | 81830 / 2.04× | 3.145 / 3.688 |
| HEVC 2560×1440@120 / 40 | intra / 1 | 961 | 40713 / 41292 / 46793 / 47701 | 120492 / 2.96× | — | 3.155 / 3.702 |
| HEVC 2560×1440@120 / 40 | intra / 2 | 961 | 40713 / 41292 / 46793 / 47701 | 120492 / 2.96× | — | 3.168 / 3.706 |
| HEVC 2560×1440@120 / 40 | cap2 / 2 | 953 | 40109 / 40920 / 46697 / 47624 | 120344 / 3.00× | 81830 / 2.04× | 3.179 / 3.734 |
| HEVC 2560×1440@120 / 40 | cap1 / 2 | 953 | 40056 / 40840 / 46687 / 47410 | 87706 / 2.19× | 44501 / 1.11× | 3.229 / 3.761 |
| HEVC 2560×1440@120 / 40 | cbr_vbv1 / 2 | 953 | 40861 / 41148 / 48109 / 58489 | 241930 / 5.92× | 121840 / 2.98× | 3.289 / 3.869 |
| HEVC 2560×1440@120 / 40 | cbr / 2 | 953 | 40896 / 40339 / 51911 / 82915 | 273504 / 6.69× | 102765 / 2.51× | 3.256 / 3.774 |
| HEVC 2560×1440@120 / 40 | default / 2 | 953 | 40275 / 41108 / 46697 / 54280 | 120344 / 2.99× | 137967 / 3.43× | 3.153 / 3.730 |
| AV1 2560×1440@120 / 40 | default / 1 | 953 | 39980 / 40456 / 45874 / 54962 | 91912 / 2.30× | 76289 / 1.91× | 2.752 / 3.606 |
| AV1 2560×1440@120 / 40 | cbr / 1 | 953 | 41005 / 41562 / 52080 / 73551 | 137697 / 3.36× | 76697 / 1.87× | 2.628 / 3.197 |
| AV1 2560×1440@120 / 40 | cbr_vbv1 / 1 | 953 | 41309 / 39539 / 48656 / 64844 | 126261 / 3.06× | 76626 / 1.85× | 2.690 / 3.236 |
| AV1 2560×1440@120 / 40 | cap1 / 1 | 953 | 39773 / 39881 / 44503 / 47864 | 64934 / 1.63× | 38589 / 0.97× | 2.658 / 3.191 |
| AV1 2560×1440@120 / 40 | cap2 / 1 | 953 | 40002 / 40268 / 45692 / 55494 | 91912 / 2.30× | 75412 / 1.89× | 2.573 / 3.137 |
| AV1 2560×1440@120 / 40 | intra / 1 | 961 | 40053 / 40482 / 45359 / 45964 | 91912 / 2.29× | — | 2.546 / 2.758 |
| AV1 2560×1440@120 / 40 | intra / 2 | 961 | 40053 / 40482 / 45359 / 45964 | 91912 / 2.29× | — | 2.561 / 3.109 |
| AV1 2560×1440@120 / 40 | cap2 / 2 | 953 | 40002 / 40268 / 45692 / 55494 | 91912 / 2.30× | 75412 / 1.89× | 2.606 / 3.142 |
| AV1 2560×1440@120 / 40 | cap1 / 2 | 953 | 39773 / 39881 / 44503 / 47864 | 64934 / 1.63× | 38589 / 0.97× | 2.665 / 3.191 |
| AV1 2560×1440@120 / 40 | cbr_vbv1 / 2 | 953 | 41309 / 39539 / 48656 / 64844 | 126261 / 3.06× | 76626 / 1.85× | 2.784 / 4.661 |
| AV1 2560×1440@120 / 40 | cbr / 2 | 953 | 41005 / 41562 / 52080 / 73551 | 137697 / 3.36× | 76697 / 1.87× | 2.565 / 3.120 |
| AV1 2560×1440@120 / 40 | default / 2 | 953 | 39980 / 40456 / 45874 / 54962 | 91912 / 2.30× | 76289 / 1.91× | 2.563 / 3.113 |
| HEVC 2560×1440@120 / 80 | default / 1 | 953 | 79432 / 79648 / 89451 / 91833 | 164372 / 2.07× | 172584 / 2.17× | 3.316 / 3.755 |
| HEVC 2560×1440@120 / 80 | cbr / 1 | 953 | 82951 / 80834 / 97724 / 114942 | 273504 / 3.30× | 139427 / 1.68× | 3.296 / 3.749 |
| HEVC 2560×1440@120 / 80 | cbr_vbv1 / 1 | 953 | 82445 / 85425 / 92433 / 96763 | 273504 / 3.32× | 179230 / 2.17× | 3.407 / 4.052 |
| HEVC 2560×1440@120 / 80 | cap1 / 1 | 953 | 79325 / 79384 / 89523 / 90888 | 149801 / 1.89× | 86920 / 1.10× | 3.318 / 3.757 |
| HEVC 2560×1440@120 / 80 | cap2 / 1 | 953 | 79396 / 79613 / 89539 / 91863 | 164372 / 2.07× | 154532 / 1.95× | 3.320 / 3.764 |
| HEVC 2560×1440@120 / 80 | intra / 1 | 961 | 79826 / 80282 / 89434 / 89686 | 164421 / 2.06× | — | 3.283 / 3.734 |
| HEVC 2560×1440@120 / 80 | intra / 2 | 961 | 79826 / 80282 / 89434 / 89686 | 164421 / 2.06× | — | 3.303 / 3.757 |
| HEVC 2560×1440@120 / 80 | cap2 / 2 | 953 | 79396 / 79613 / 89539 / 91863 | 164372 / 2.07× | 154532 / 1.95× | 3.350 / 3.783 |
| HEVC 2560×1440@120 / 80 | cap1 / 2 | 953 | 79325 / 79384 / 89523 / 90888 | 149801 / 1.89× | 86920 / 1.10× | 3.376 / 3.793 |
| HEVC 2560×1440@120 / 80 | cbr_vbv1 / 2 | 953 | 82445 / 85425 / 92433 / 96763 | 273504 / 3.32× | 179230 / 2.17× | 3.347 / 3.800 |
| HEVC 2560×1440@120 / 80 | cbr / 2 | 953 | 82951 / 80834 / 97724 / 114942 | 273504 / 3.30× | 139427 / 1.68× | 3.309 / 3.754 |
| HEVC 2560×1440@120 / 80 | default / 2 | 953 | 79432 / 79648 / 89451 / 91833 | 164372 / 2.07× | 172584 / 2.17× | 3.277 / 3.738 |
| AV1 2560×1440@120 / 80 | default / 1 | 953 | 79251 / 79181 / 90271 / 96057 | 115988 / 1.46× | 129240 / 1.63× | 2.617 / 3.144 |
| AV1 2560×1440@120 / 80 | cbr / 1 | 953 | 82938 / 81195 / 95358 / 121335 | 137697 / 1.66× | 112415 / 1.36× | 2.605 / 3.150 |
| AV1 2560×1440@120 / 80 | cbr_vbv1 / 1 | 953 | 82452 / 78778 / 93965 / 113246 | 137697 / 1.67× | 119465 / 1.45× | 2.745 / 3.226 |
| AV1 2560×1440@120 / 80 | cap1 / 1 | 953 | 79122 / 79041 / 88929 / 94888 | 105266 / 1.33× | 79608 / 1.01× | 2.734 / 3.259 |
| AV1 2560×1440@120 / 80 | cap2 / 1 | 953 | 79251 / 79181 / 90271 / 96057 | 115988 / 1.46× | 129240 / 1.63× | 2.759 / 3.275 |
| AV1 2560×1440@120 / 80 | intra / 1 | 961 | 79638 / 80068 / 90338 / 93959 | 115988 / 1.46× | — | 2.712 / 3.209 |
| AV1 2560×1440@120 / 80 | intra / 2 | 961 | 79638 / 80068 / 90338 / 93959 | 115988 / 1.46× | — | 2.655 / 3.188 |
| AV1 2560×1440@120 / 80 | cap2 / 2 | 953 | 79251 / 79181 / 90271 / 96057 | 115988 / 1.46× | 129240 / 1.63× | 2.643 / 3.188 |
| AV1 2560×1440@120 / 80 | cap1 / 2 | 953 | 79122 / 79041 / 88929 / 94888 | 105266 / 1.33× | 79608 / 1.01× | 2.630 / 3.181 |
| AV1 2560×1440@120 / 80 | cbr_vbv1 / 2 | 953 | 82452 / 78778 / 93965 / 113246 | 137697 / 1.67× | 119465 / 1.45× | 2.673 / 3.184 |
| AV1 2560×1440@120 / 80 | cbr / 2 | 953 | 82938 / 81195 / 95358 / 121335 | 137697 / 1.66× | 112415 / 1.36× | 2.578 / 3.134 |
| AV1 2560×1440@120 / 80 | default / 2 | 953 | 79251 / 79181 / 90271 / 96057 | 115988 / 1.46× | 129240 / 1.63× | 2.621 / 3.171 |
| HEVC 3840×2160@60 / 20 | default / 1 | 473 | 41318 / 39558 / 49226 / 50142 | 133527 / 3.23× | 113784 / 2.75× | 5.959 / 6.354 |
| HEVC 3840×2160@60 / 20 | cbr / 1 | 473 | 40300 / 37894 / 50655 / 53448 | 563110 / 13.97× | 102447 / 2.54× | 5.940 / 6.458 |
| HEVC 3840×2160@60 / 20 | cbr_vbv1 / 1 | 473 | 41254 / 37898 / 51685 / 53500 | 321254 / 7.79× | 102447 / 2.48× | 5.992 / 6.512 |
| HEVC 3840×2160@60 / 20 | cap1 / 1 | 473 | 41572 / 39799 / 50024 / 50212 | 90517 / 2.18× | 44496 / 1.07× | 5.971 / 6.369 |
| HEVC 3840×2160@60 / 20 | cap2 / 1 | 473 | 41256 / 39771 / 49265 / 50142 | 133527 / 3.24× | 84020 / 2.04× | 5.989 / 6.353 |
| HEVC 3840×2160@60 / 20 | intra / 1 | 481 | 41830 / 40919 / 49278 / 49351 | 133416 / 3.19× | — | 5.824 / 6.282 |
| HEVC 3840×2160@60 / 20 | intra / 2 | 481 | 41830 / 40919 / 49278 / 49351 | 133416 / 3.19× | — | 5.878 / 6.425 |
| HEVC 3840×2160@60 / 20 | cap2 / 2 | 473 | 41256 / 39771 / 49265 / 50142 | 133527 / 3.24× | 84020 / 2.04× | 6.034 / 6.443 |
| HEVC 3840×2160@60 / 20 | cap1 / 2 | 473 | 41572 / 39799 / 50024 / 50212 | 90517 / 2.18× | 44496 / 1.07× | 5.996 / 6.637 |
| HEVC 3840×2160@60 / 20 | cbr_vbv1 / 2 | 473 | 41254 / 37898 / 51685 / 53500 | 321254 / 7.79× | 102447 / 2.48× | 5.985 / 6.531 |
| HEVC 3840×2160@60 / 20 | cbr / 2 | 473 | 40300 / 37894 / 50655 / 53448 | 563110 / 13.97× | 102447 / 2.54× | 5.977 / 6.528 |
| HEVC 3840×2160@60 / 20 | default / 2 | 473 | 41318 / 39558 / 49226 / 50142 | 133527 / 3.23× | 113784 / 2.75× | 5.985 / 6.676 |
| AV1 3840×2160@60 / 20 | default / 1 | 473 | 41001 / 41103 / 48304 / 48958 | 106459 / 2.60× | 90172 / 2.20× | 4.834 / 5.327 |
| AV1 3840×2160@60 / 20 | cbr / 1 | 473 | 40984 / 38304 / 63241 / 74613 | 285815 / 6.97× | 91166 / 2.22× | 4.939 / 5.500 |
| AV1 3840×2160@60 / 20 | cbr_vbv1 / 1 | 473 | 41184 / 41061 / 53037 / 53149 | 181936 / 4.42× | 102945 / 2.50× | 4.918 / 5.380 |
| AV1 3840×2160@60 / 20 | cap1 / 1 | 473 | 41263 / 41379 / 48755 / 50545 | 68709 / 1.67× | 45928 / 1.11× | 4.923 / 5.389 |
| AV1 3840×2160@60 / 20 | cap2 / 1 | 473 | 41063 / 41246 / 48492 / 49312 | 106459 / 2.59× | 82943 / 2.02× | 4.938 / 5.547 |
| AV1 3840×2160@60 / 20 | intra / 1 | 481 | 41528 / 41283 / 49533 / 50787 | 106459 / 2.56× | — | 5.041 / 5.778 |
| AV1 3840×2160@60 / 20 | intra / 2 | 481 | 41528 / 41283 / 49533 / 50787 | 106459 / 2.56× | — | 4.994 / 5.718 |
| AV1 3840×2160@60 / 20 | cap2 / 2 | 473 | 41063 / 41246 / 48492 / 49312 | 106459 / 2.59× | 82943 / 2.02× | 5.050 / 5.779 |
| AV1 3840×2160@60 / 20 | cap1 / 2 | 473 | 41263 / 41379 / 48755 / 50545 | 68709 / 1.67× | 45928 / 1.11× | 5.037 / 5.524 |
| AV1 3840×2160@60 / 20 | cbr_vbv1 / 2 | 473 | 41184 / 41061 / 53037 / 53149 | 181936 / 4.42× | 102945 / 2.50× | 5.033 / 5.671 |
| AV1 3840×2160@60 / 20 | cbr / 2 | 473 | 40984 / 38304 / 63241 / 74613 | 285815 / 6.97× | 91166 / 2.22× | 5.051 / 5.725 |
| AV1 3840×2160@60 / 20 | default / 2 | 473 | 41001 / 41103 / 48304 / 48958 | 106459 / 2.60× | 90172 / 2.20× | 5.042 / 5.773 |
| HEVC 3840×2160@60 / 40 | default / 1 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.216 / 6.771 |
| HEVC 3840×2160@60 / 40 | cbr / 1 | 473 | 80935 / 78090 / 143341 / 158445 | 615045 / 7.60× | 245278 / 3.03× | 6.210 / 6.789 |
| HEVC 3840×2160@60 / 40 | cbr_vbv1 / 1 | 473 | 79922 / 78940 / 100353 / 112325 | 512986 / 6.42× | 253462 / 3.17× | 6.050 / 6.721 |
| HEVC 3840×2160@60 / 40 | cap1 / 1 | 473 | 79397 / 78767 / 93307 / 93482 | 178313 / 2.25× | 89626 / 1.13× | 6.103 / 6.752 |
| HEVC 3840×2160@60 / 40 | cap2 / 1 | 473 | 79425 / 78987 / 93313 / 93857 | 245474 / 3.09× | 167074 / 2.10× | 6.083 / 6.716 |
| HEVC 3840×2160@60 / 40 | intra / 1 | 481 | 82048 / 80867 / 93426 / 93797 | 245658 / 2.99× | — | 6.071 / 6.548 |
| HEVC 3840×2160@60 / 40 | intra / 2 | 481 | 82048 / 80867 / 93426 / 93797 | 245658 / 2.99× | — | 6.070 / 6.488 |
| HEVC 3840×2160@60 / 40 | cap2 / 2 | 473 | 79425 / 78987 / 93313 / 93857 | 245474 / 3.09× | 167074 / 2.10× | 6.034 / 6.395 |
| HEVC 3840×2160@60 / 40 | cap1 / 2 | 473 | 79397 / 78767 / 93307 / 93482 | 178313 / 2.25× | 89626 / 1.13× | 6.005 / 6.386 |
| HEVC 3840×2160@60 / 40 | cbr_vbv1 / 2 | 473 | 79922 / 78940 / 100353 / 112325 | 512986 / 6.42× | 253462 / 3.17× | 5.991 / 6.359 |
| HEVC 3840×2160@60 / 40 | cbr / 2 | 473 | 80935 / 78090 / 143341 / 158445 | 615045 / 7.60× | 245278 / 3.03× | 5.954 / 6.396 |
| HEVC 3840×2160@60 / 40 | default / 2 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.007 / 6.376 |
| AV1 3840×2160@60 / 40 | default / 1 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 5.079 / 5.774 |
| AV1 3840×2160@60 / 40 | cbr / 1 | 473 | 82148 / 81758 / 128763 / 141099 | 307290 / 3.74× | 162112 / 1.97× | 4.945 / 5.659 |
| AV1 3840×2160@60 / 40 | cbr_vbv1 / 1 | 473 | 82217 / 79758 / 124565 / 127992 | 266944 / 3.25× | 164820 / 2.00× | 4.923 / 5.737 |
| AV1 3840×2160@60 / 40 | cap1 / 1 | 473 | 78366 / 78989 / 93744 / 94929 | 132770 / 1.69× | 82356 / 1.05× | 4.982 / 5.708 |
| AV1 3840×2160@60 / 40 | cap2 / 1 | 473 | 79150 / 79293 / 105441 / 111427 | 185400 / 2.34× | 152792 / 1.93× | 4.865 / 5.665 |
| AV1 3840×2160@60 / 40 | intra / 1 | 481 | 80191 / 79271 / 93847 / 94157 | 185400 / 2.31× | — | 4.997 / 5.805 |
| AV1 3840×2160@60 / 40 | intra / 2 | 481 | 80191 / 79271 / 93847 / 94157 | 185400 / 2.31× | — | 4.929 / 5.724 |
| AV1 3840×2160@60 / 40 | cap2 / 2 | 473 | 79150 / 79293 / 105441 / 111427 | 185400 / 2.34× | 152792 / 1.93× | 5.007 / 5.660 |
| AV1 3840×2160@60 / 40 | cap1 / 2 | 473 | 78366 / 78989 / 93744 / 94929 | 132770 / 1.69× | 82356 / 1.05× | 4.914 / 5.314 |
| AV1 3840×2160@60 / 40 | cbr_vbv1 / 2 | 473 | 82217 / 79758 / 124565 / 127992 | 266944 / 3.25× | 164820 / 2.00× | 5.001 / 5.591 |
| AV1 3840×2160@60 / 40 | cbr / 2 | 473 | 82148 / 81758 / 128763 / 141099 | 307290 / 3.74× | 162112 / 1.97× | 5.024 / 5.798 |
| AV1 3840×2160@60 / 40 | default / 2 **† stream/probe overlap** | 448 | 79360 / 79284 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 6.423 / 12.266 |
| HEVC 3840×2160@60 / 80 | default / 1 **† stream/probe overlap** | 473 | 156383 / 155179 / 180262 / 182161 | 358474 / 2.29× | 324108 / 2.07× | 6.136 / 6.645 |
| HEVC 3840×2160@60 / 80 | cbr / 1 **† stream/probe overlap** | 473 | 164289 / 162660 / 215923 / 216938 | 615045 / 3.74× | 315279 / 1.92× | 6.114 / 6.806 |
| HEVC 3840×2160@60 / 80 | cbr_vbv1 / 1 **† stream/probe overlap** | 473 | 163989 / 160788 / 195259 / 199848 | 615045 / 3.75× | 340352 / 2.08× | 6.157 / 6.766 |
| HEVC 3840×2160@60 / 80 | cap1 / 1 **† stream/probe overlap** | 441 | 154612 / 152135 / 181661 / 182206 | 308507 / 2.00× | 170031 / 1.10× | 8.273 / 64.236 |
| HEVC 3840×2160@60 / 80 | cap2 / 1 | 473 | 156500 / 154927 / 180484 / 182115 | 358474 / 2.29× | 315456 / 2.02× | 6.156 / 6.750 |
| HEVC 3840×2160@60 / 80 | intra / 1 | 481 | 158262 / 161579 / 181698 / 185403 | 358503 / 2.27× | — | 6.136 / 6.757 |
| HEVC 3840×2160@60 / 80 | intra / 2 | 481 | 158262 / 161579 / 181698 / 185403 | 358503 / 2.27× | — | 6.127 / 6.731 |
| HEVC 3840×2160@60 / 80 | cap2 / 2 | 473 | 156500 / 154927 / 180484 / 182115 | 358474 / 2.29× | 315456 / 2.02× | 6.008 / 6.704 |
| HEVC 3840×2160@60 / 80 | cap1 / 2 | 473 | 154374 / 151574 / 181661 / 182206 | 308507 / 2.00× | 170031 / 1.10× | 6.108 / 6.749 |
| HEVC 3840×2160@60 / 80 | cbr_vbv1 / 2 | 473 | 163989 / 160788 / 195259 / 199848 | 615045 / 3.75× | 340352 / 2.08× | 6.240 / 6.836 |
| HEVC 3840×2160@60 / 80 | cbr / 2 | 473 | 164289 / 162660 / 215923 / 216938 | 615045 / 3.74× | 315279 / 1.92× | 6.242 / 6.853 |
| HEVC 3840×2160@60 / 80 | default / 2 | 473 | 156383 / 155179 / 180262 / 182161 | 358474 / 2.29× | 324108 / 2.07× | 6.126 / 6.779 |
| AV1 3840×2160@60 / 80 | default / 1 | 473 | 154544 / 155726 / 173910 / 187397 | 253332 / 1.64× | 255986 / 1.66× | 4.804 / 5.318 |
| AV1 3840×2160@60 / 80 | cbr / 1 | 473 | 165737 / 162718 / 228771 / 251926 | 307290 / 1.85× | 250269 / 1.51× | 4.996 / 5.728 |
| AV1 3840×2160@60 / 80 | cbr_vbv1 / 1 | 473 | 164476 / 161764 / 208183 / 226477 | 307290 / 1.87× | 250464 / 1.52× | 4.935 / 5.770 |
| AV1 3840×2160@60 / 80 | cap1 / 1 | 473 | 153742 / 154901 / 174143 / 176661 | 215311 / 1.40× | 157031 / 1.02× | 4.900 / 5.750 |
| AV1 3840×2160@60 / 80 | cap2 / 1 | 473 | 154544 / 155726 / 173910 / 187397 | 253332 / 1.64× | 255986 / 1.66× | 4.924 / 5.730 |
| AV1 3840×2160@60 / 80 | intra / 1 | 481 | 155089 / 156143 / 172484 / 175581 | 253332 / 1.63× | — | 4.975 / 5.759 |
| AV1 3840×2160@60 / 80 | intra / 2 | 481 | 155089 / 156143 / 172484 / 175581 | 253332 / 1.63× | — | 4.984 / 5.482 |
| AV1 3840×2160@60 / 80 | cap2 / 2 | 473 | 154544 / 155726 / 173910 / 187397 | 253332 / 1.64× | 255986 / 1.66× | 5.006 / 5.894 |
| AV1 3840×2160@60 / 80 | cap1 / 2 | 473 | 153742 / 154901 / 174143 / 176661 | 215311 / 1.40× | 157031 / 1.02× | 4.952 / 5.736 |
| AV1 3840×2160@60 / 80 | cbr_vbv1 / 2 | 473 | 164476 / 161764 / 208183 / 226477 | 307290 / 1.87× | 250464 / 1.52× | 4.949 / 5.451 |
| AV1 3840×2160@60 / 80 | cbr / 2 | 473 | 165737 / 162718 / 228771 / 251926 | 307290 / 1.85× | 250269 / 1.51× | 4.961 / 5.539 |
| AV1 3840×2160@60 / 80 | default / 2 | 473 | 154544 / 155726 / 173910 / 187397 | 253332 / 1.64× | 255986 / 1.66× | 4.865 / 5.358 |

### Frequent recovery requests at 4K60 and 63 Mbps

The same release method, with `--bitrate 63000 --idr-interval 6`, requests
ten recovery keyframes per second, approximating frequent loss reports
without injecting network loss. `cbr_cap1` and `cbr_cap2` add the respective
cap to `amd_rc=cbr` and `amd_vbv_buffer_frames=1`.

| Mode / Mbps | Setting / run | Non-keyframes | Steady mean / p50 / p99 / max (bytes) | Startup bytes / steady mean | Recovery max bytes / steady mean | Encode mean / p99 (ms) |
|---|---|---:|---|---:|---|---|
| HEVC 3840×2160@60 / 63 | default / 1 | 401 | 109013 / 106784 / 139821 / 142313 | 334673 / 3.07× | 294072 / 2.70× | 5.924 / 6.351 |
| HEVC 3840×2160@60 / 63 | cbr_vbv1 / 1 | 401 | 114254 / 108384 / 165169 / 165169 | 615045 / 5.38× | 301985 / 2.64× | 6.083 / 6.736 |
| HEVC 3840×2160@60 / 63 | cap1 / 1 | 401 | 101273 / 100411 / 119624 / 140142 | 254477 / 2.51× | 136605 / 1.35× | 5.971 / 6.585 |
| HEVC 3840×2160@60 / 63 | cap2 / 1 | 401 | 108302 / 104644 / 136414 / 140626 | 334673 / 3.09× | 254958 / 2.35× | 5.997 / 6.669 |
| HEVC 3840×2160@60 / 63 | cbr_cap1 / 1 | 401 | 107310 / 101679 / 145249 / 162267 | 448837 / 4.18× | 186982 / 1.74× | 5.971 / 6.419 |
| HEVC 3840×2160@60 / 63 | cbr_cap2 / 1 | 401 | 113592 / 106497 / 159120 / 165644 | 615045 / 5.41× | 271885 / 2.39× | 6.106 / 6.834 |
| HEVC 3840×2160@60 / 63 | cbr_cap2 / 2 | 401 | 113592 / 106497 / 159120 / 165644 | 615045 / 5.41× | 271885 / 2.39× | 6.048 / 6.608 |
| HEVC 3840×2160@60 / 63 | cbr_cap1 / 2 | 401 | 107310 / 101679 / 145249 / 162267 | 448837 / 4.18× | 186982 / 1.74× | 5.890 / 6.519 |
| HEVC 3840×2160@60 / 63 | cap2 / 2 | 401 | 108302 / 104644 / 136414 / 140626 | 334673 / 3.09× | 254958 / 2.35× | 5.956 / 6.738 |
| HEVC 3840×2160@60 / 63 | cap1 / 2 | 401 | 101273 / 100411 / 119624 / 140142 | 254477 / 2.51× | 136605 / 1.35× | 6.078 / 6.822 |
| HEVC 3840×2160@60 / 63 | cbr_vbv1 / 2 | 401 | 114254 / 108384 / 165169 / 165169 | 615045 / 5.38× | 301985 / 2.64× | 6.014 / 6.678 |
| HEVC 3840×2160@60 / 63 | default / 2 | 401 | 109013 / 106784 / 139821 / 142313 | 334673 / 3.07× | 294072 / 2.70× | 5.910 / 6.486 |
| AV1 3840×2160@60 / 63 | default / 1 | 401 | 116619 / 115511 / 148503 / 148606 | 243277 / 2.09× | 213511 / 1.83× | 4.763 / 5.303 |
| AV1 3840×2160@60 / 63 | cbr_vbv1 / 1 | 401 | 123402 / 123240 / 162894 / 162894 | 307290 / 2.49× | 203733 / 1.65× | 4.810 / 5.257 |
| AV1 3840×2160@60 / 63 | cap1 / 1 | 401 | 110072 / 109551 / 135051 / 140585 | 180535 / 1.64× | 131815 / 1.20× | 4.790 / 5.234 |
| AV1 3840×2160@60 / 63 | cap2 / 1 | 401 | 116619 / 115511 / 148503 / 148606 | 243277 / 2.09× | 213511 / 1.83× | 4.864 / 5.291 |
| AV1 3840×2160@60 / 63 | cbr_cap1 / 1 | 401 | 118700 / 117256 / 155871 / 155871 | 244193 / 2.06× | 161413 / 1.36× | 5.190 / 5.813 |
| AV1 3840×2160@60 / 63 | cbr_cap2 / 1 | 401 | 123402 / 123240 / 162894 / 162894 | 307290 / 2.49× | 203733 / 1.65× | 4.947 / 6.385 |
| AV1 3840×2160@60 / 63 | cbr_cap2 / 2 | 401 | 123402 / 123240 / 162894 / 162894 | 307290 / 2.49× | 203733 / 1.65× | 4.935 / 5.336 |
| AV1 3840×2160@60 / 63 | cbr_cap1 / 2 | 401 | 118700 / 117256 / 155871 / 155871 | 244193 / 2.06× | 161413 / 1.36× | 4.917 / 5.376 |
| AV1 3840×2160@60 / 63 | cap2 / 2 | 401 | 116619 / 115511 / 148503 / 148606 | 243277 / 2.09× | 213511 / 1.83× | 4.944 / 5.702 |
| AV1 3840×2160@60 / 63 | cap1 / 2 | 401 | 110072 / 109551 / 135051 / 140585 | 180535 / 1.64× | 131815 / 1.20× | 4.904 / 5.562 |
| AV1 3840×2160@60 / 63 | cbr_vbv1 / 2 | 401 | 123402 / 123240 / 162894 / 162894 | 307290 / 2.49× | 203733 / 1.65× | 4.989 / 5.449 |
| AV1 3840×2160@60 / 63 | default / 2 | 401 | 116619 / 115511 / 148503 / 148606 | 243277 / 2.09× | 213511 / 1.83× | 4.987 / 5.696 |

### Peak and VBV recheck after two seconds of warmup

The development screening's short warmup could confound VBV comparisons.
These release runs repeat the peak and buffer options at 4K60/40 Mbps with
the same two-second warmup as the main matrix. They also check HRD, the
four-frame cap, and intra refresh with explicit recovery requests.

| Mode / Mbps | Setting / run | Non-keyframes | Steady mean / p50 / p99 / max (bytes) | Startup bytes / steady mean | Recovery max bytes / steady mean | Encode mean / p99 (ms) |
|---|---|---:|---|---:|---|---|
| HEVC 3840×2160@60 / 40 | default / 1 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 5.957 / 6.621 |
| HEVC 3840×2160@60 / 40 | peak1 / 1 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.056 / 6.575 |
| HEVC 3840×2160@60 / 40 | peak15 / 1 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.014 / 6.453 |
| HEVC 3840×2160@60 / 40 | peak2 / 1 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.027 / 6.463 |
| HEVC 3840×2160@60 / 40 | vbv05 / 1 | 473 | 78485 / 78020 / 93492 / 117456 | 213837 / 2.72× | 296535 / 3.78× | 6.011 / 6.385 |
| HEVC 3840×2160@60 / 40 | vbv1 / 1 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.025 / 6.396 |
| HEVC 3840×2160@60 / 40 | vbv2 / 1 | 473 | 80653 / 80187 / 98329 / 105790 | 293486 / 3.64× | 242837 / 3.01× | 6.050 / 6.615 |
| HEVC 3840×2160@60 / 40 | cap4 / 1 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.073 / 6.458 |
| HEVC 3840×2160@60 / 40 | hrd / 1 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.124 / 6.632 |
| HEVC 3840×2160@60 / 40 | cap2hrd / 1 | 473 | 79425 / 78987 / 93313 / 93857 | 245474 / 3.09× | 167074 / 2.10× | 6.121 / 6.679 |
| HEVC 3840×2160@60 / 40 | intra_idr / 1 | 473 | 79829 / 79143 / 92769 / 96778 | 245658 / 3.08× | 266805 / 3.34× | 6.104 / 6.496 |
| HEVC 3840×2160@60 / 40 | intra_idr / 2 | 473 | 79829 / 79143 / 92769 / 96778 | 245658 / 3.08× | 266805 / 3.34× | 6.042 / 6.773 |
| HEVC 3840×2160@60 / 40 | cap2hrd / 2 | 473 | 79425 / 78987 / 93313 / 93857 | 245474 / 3.09× | 167074 / 2.10× | 6.092 / 6.544 |
| HEVC 3840×2160@60 / 40 | hrd / 2 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.032 / 6.679 |
| HEVC 3840×2160@60 / 40 | cap4 / 2 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.059 / 6.771 |
| HEVC 3840×2160@60 / 40 | vbv2 / 2 | 473 | 80653 / 80187 / 98329 / 105790 | 293486 / 3.64× | 242837 / 3.01× | 6.158 / 6.732 |
| HEVC 3840×2160@60 / 40 | vbv1 / 2 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.130 / 6.660 |
| HEVC 3840×2160@60 / 40 | vbv05 / 2 | 473 | 78485 / 78020 / 93492 / 117456 | 213837 / 2.72× | 296535 / 3.78× | 6.129 / 6.718 |
| HEVC 3840×2160@60 / 40 | peak2 / 2 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.052 / 6.624 |
| HEVC 3840×2160@60 / 40 | peak15 / 2 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.054 / 6.537 |
| HEVC 3840×2160@60 / 40 | peak1 / 2 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.202 / 6.730 |
| HEVC 3840×2160@60 / 40 | default / 2 | 473 | 79820 / 79234 / 93258 / 96964 | 245474 / 3.08× | 265995 / 3.33× | 6.202 / 6.748 |
| AV1 3840×2160@60 / 40 | default / 1 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.871 / 5.325 |
| AV1 3840×2160@60 / 40 | peak1 / 1 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 5.021 / 5.767 |
| AV1 3840×2160@60 / 40 | peak15 / 1 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.960 / 5.752 |
| AV1 3840×2160@60 / 40 | peak2 / 1 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.773 / 5.304 |
| AV1 3840×2160@60 / 40 | vbv05 / 1 | 473 | 78508 / 79142 / 93829 / 102300 | 158745 / 2.02× | 163561 / 2.08× | 4.937 / 5.512 |
| AV1 3840×2160@60 / 40 | vbv1 / 1 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.959 / 5.559 |
| AV1 3840×2160@60 / 40 | vbv2 / 1 | 473 | 79425 / 79431 / 105676 / 114494 | 225386 / 2.84× | 153760 / 1.94× | 4.901 / 5.604 |
| AV1 3840×2160@60 / 40 | cap4 / 1 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.899 / 5.736 |
| AV1 3840×2160@60 / 40 | hrd / 1 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.901 / 5.292 |
| AV1 3840×2160@60 / 40 | cap2hrd / 1 | 473 | 79150 / 79293 / 105441 / 111427 | 185400 / 2.34× | 152792 / 1.93× | 4.854 / 5.718 |
| AV1 3840×2160@60 / 40 | intra_idr / 1 | 473 | 78990 / 79133 / 105404 / 112083 | 185400 / 2.35× | 153531 / 1.94× | 5.000 / 5.732 |
| AV1 3840×2160@60 / 40 | intra_idr / 2 | 473 | 78990 / 79133 / 105404 / 112083 | 185400 / 2.35× | 153531 / 1.94× | 4.997 / 5.762 |
| AV1 3840×2160@60 / 40 | cap2hrd / 2 | 473 | 79150 / 79293 / 105441 / 111427 | 185400 / 2.34× | 152792 / 1.93× | 4.922 / 5.750 |
| AV1 3840×2160@60 / 40 | hrd / 2 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.814 / 5.266 |
| AV1 3840×2160@60 / 40 | cap4 / 2 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.876 / 5.363 |
| AV1 3840×2160@60 / 40 | vbv2 / 2 | 473 | 79425 / 79431 / 105676 / 114494 | 225386 / 2.84× | 153760 / 1.94× | 4.808 / 5.337 |
| AV1 3840×2160@60 / 40 | vbv1 / 2 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.967 / 5.374 |
| AV1 3840×2160@60 / 40 | vbv05 / 2 | 473 | 78508 / 79142 / 93829 / 102300 | 158745 / 2.02× | 163561 / 2.08× | 4.786 / 5.316 |
| AV1 3840×2160@60 / 40 | peak2 / 2 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.951 / 5.498 |
| AV1 3840×2160@60 / 40 | peak15 / 2 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.942 / 5.661 |
| AV1 3840×2160@60 / 40 | peak1 / 2 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.954 / 5.670 |
| AV1 3840×2160@60 / 40 | default / 2 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.929 / 5.692 |

### Repeats of modes affected by the live stream

Both affected mode groups were repeated in full, again in forward/reverse
order. All 24 runs below had no active stream or other observed performance
probe, completed 481 of 481 submissions, and reached the requested cadence.
Use these clean comparisons for AV1 4K60/40 Mbps and HEVC 4K60/80 Mbps;
the marked original rows remain above for transparency.

| Mode / Mbps | Setting / run | Non-keyframes | Steady mean / p50 / p99 / max (bytes) | Startup bytes / steady mean | Recovery max bytes / steady mean | Encode mean / p99 (ms) |
|---|---|---:|---|---:|---|---|
| AV1 3840×2160@60 / 40 | default / 1 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.904 / 5.454 |
| AV1 3840×2160@60 / 40 | cbr / 1 | 473 | 82148 / 81758 / 128763 / 141099 | 307290 / 3.74× | 162112 / 1.97× | 5.026 / 5.794 |
| AV1 3840×2160@60 / 40 | cbr_vbv1 / 1 | 473 | 82217 / 79758 / 124565 / 127992 | 266944 / 3.25× | 164820 / 2.00× | 5.027 / 5.771 |
| AV1 3840×2160@60 / 40 | cap1 / 1 | 473 | 78366 / 78989 / 93744 / 94929 | 132770 / 1.69× | 82356 / 1.05× | 4.950 / 5.321 |
| AV1 3840×2160@60 / 40 | cap2 / 1 | 473 | 79150 / 79293 / 105441 / 111427 | 185400 / 2.34× | 152792 / 1.93× | 4.969 / 5.695 |
| AV1 3840×2160@60 / 40 | intra / 1 | 481 | 80191 / 79271 / 93847 / 94157 | 185400 / 2.31× | — | 4.904 / 5.353 |
| AV1 3840×2160@60 / 40 | intra / 2 | 481 | 80191 / 79271 / 93847 / 94157 | 185400 / 2.31× | — | 4.969 / 5.414 |
| AV1 3840×2160@60 / 40 | cap2 / 2 | 473 | 79150 / 79293 / 105441 / 111427 | 185400 / 2.34× | 152792 / 1.93× | 4.943 / 5.427 |
| AV1 3840×2160@60 / 40 | cap1 / 2 | 473 | 78366 / 78989 / 93744 / 94929 | 132770 / 1.69× | 82356 / 1.05× | 4.938 / 5.423 |
| AV1 3840×2160@60 / 40 | cbr_vbv1 / 2 | 473 | 82217 / 79758 / 124565 / 127992 | 266944 / 3.25× | 164820 / 2.00× | 4.897 / 5.289 |
| AV1 3840×2160@60 / 40 | cbr / 2 | 473 | 82148 / 81758 / 128763 / 141099 | 307290 / 3.74× | 162112 / 1.97× | 4.883 / 5.367 |
| AV1 3840×2160@60 / 40 | default / 2 | 473 | 79254 / 79331 / 105540 / 112372 | 185400 / 2.34× | 154097 / 1.94× | 4.893 / 5.370 |
| HEVC 3840×2160@60 / 80 | default / 1 | 473 | 156383 / 155179 / 180262 / 182161 | 358474 / 2.29× | 324108 / 2.07× | 6.141 / 6.724 |
| HEVC 3840×2160@60 / 80 | cbr / 1 | 473 | 164289 / 162660 / 215923 / 216938 | 615045 / 3.74× | 315279 / 1.92× | 6.127 / 6.700 |
| HEVC 3840×2160@60 / 80 | cbr_vbv1 / 1 | 473 | 163989 / 160788 / 195259 / 199848 | 615045 / 3.75× | 340352 / 2.08× | 6.173 / 6.726 |
| HEVC 3840×2160@60 / 80 | cap1 / 1 | 473 | 154374 / 151574 / 181661 / 182206 | 308507 / 2.00× | 170031 / 1.10× | 6.066 / 6.718 |
| HEVC 3840×2160@60 / 80 | cap2 / 1 | 473 | 156500 / 154927 / 180484 / 182115 | 358474 / 2.29× | 315456 / 2.02× | 6.081 / 6.560 |
| HEVC 3840×2160@60 / 80 | intra / 1 | 481 | 158262 / 161579 / 181698 / 185403 | 358503 / 2.27× | — | 6.138 / 6.697 |
| HEVC 3840×2160@60 / 80 | intra / 2 | 481 | 158262 / 161579 / 181698 / 185403 | 358503 / 2.27× | — | 6.096 / 6.789 |
| HEVC 3840×2160@60 / 80 | cap2 / 2 | 473 | 156500 / 154927 / 180484 / 182115 | 358474 / 2.29× | 315456 / 2.02× | 6.146 / 6.776 |
| HEVC 3840×2160@60 / 80 | cap1 / 2 | 473 | 154374 / 151574 / 181661 / 182206 | 308507 / 2.00× | 170031 / 1.10× | 6.132 / 6.657 |
| HEVC 3840×2160@60 / 80 | cbr_vbv1 / 2 | 473 | 163989 / 160788 / 195259 / 199848 | 615045 / 3.75× | 340352 / 2.08× | 6.151 / 6.801 |
| HEVC 3840×2160@60 / 80 | cbr / 2 | 473 | 164289 / 162660 / 215923 / 216938 | 615045 / 3.74× | 315279 / 1.92× | 6.159 / 6.800 |
| HEVC 3840×2160@60 / 80 | default / 2 | 473 | 156383 / 155179 / 180262 / 182161 | 358474 / 2.29× | 324108 / 2.07× | 6.135 / 6.739 |

### Decision and limits

Keep `vbr_latency`, ultra-low-latency usage, speed, VBAQ on and HRD off as
the defaults. Peak bitrate, VBV and the maximum frame size remain driver
defaults (`0` in the three new settings). No input-queue, internal-latency,
lookahead, filler or frame-skipping setting is added to implement a cap.
The measurements do not justify a universal change that both removes
bursts and preserves encode time and picture quality across these modes.

At 4K60/40 Mbps HEVC its non-keyframe p99 is 143,341 bytes against VBR's
93,258; explicitly restoring a one-frame VBV reduces that to 100,353. At
1080p60/80 Mbps CBR improves that comparison. At 4K60/20 Mbps AV1, CBR with a
one-frame VBV instead increases recovery maximum by 14.2%. Imported
`amd_rc=cbr` settings are not overwritten.

Explicit 1×, 1.5× and 2× peak requests read back correctly but produce
identical frame sizes in the tested latency-constrained VBR mode. One frame
of explicit VBV reproduces the default. Half a frame increases HEVC's
4K40 recovery maximum from 265,995 to 296,535 bytes; two frames reduce it
to 242,837 but raise non-keyframe p99 from 93,258 to 98,329. HRD adds no
size reduction in these tests, including when paired with a two-frame cap.

The frame cap is useful as an **opt-in trade-off**, not a hard bound.
`amd_max_frame_size=1` reduces 4K40 HEVC recovery maximum from 265,995 to
89,626 bytes (66.3%). Non-keyframe p99 barely changes, 93,258 to 93,307.
The two mean encode times are 6.216/6.007 ms without the cap and
6.103/6.005 ms with it. A two-frame cap gives 167,074 bytes (37.2% smaller)
and 6.083/6.034 ms. On AV1 a two-frame cap often does nothing because its
keyframes already fit that budget; the one-frame cap has a larger effect.

With ten recovery requests per second at 4K60/63 Mbps, HEVC VBR's maximum falls
from 294,072 to 136,605 bytes with a one-frame cap, but measured encoded
payload also falls from 64.12 to 51.01 Mbps. Mean encode time is 5.924/5.910 ms
uncapped and 5.971/6.078 ms capped: a small **0.108 ms increase** in the
two-run mean. CBR plus a one-frame VBV and one-frame cap still reaches 186,982
bytes for a recovery frame and 448,837 at startup.

Even outside the frequent-recovery test, the cap can be exceeded. The 4K40 HEVC
one-frame request is 83,333 bytes, but its startup frame is 178,313 bytes. Some
ordinary frames exceed it too. All new properties were accepted on this RX 7900
XT; that is not RDNA4 validation. The previous RDNA4 freeze reports concern
`LowLatencyInternal` and `InputQueueSize`; this change forces neither.

Intra refresh stays client-negotiated. With no forced keyframes it avoids the
periodic spikes, but explicitly requesting recovery still produces a full
keyframe: 266,805 bytes for HEVC with refresh versus 265,995 without at 4K40.
The HEVC policy refreshes seven of 2,040 CTBs per slot at 4K. The [AMF HEVC
API](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/doc/AMF_Video_Encode_HEVC_API.md)
defines that property in 64×64 CTBs per slot. Replacing a decoder-reset
keyframe request with that gradual process is not justified by this test.

For an affected AMD user who can compare picture quality, the new setting
allows testing a one- or two-frame cap with the existing VBR default. It
does not promise that an arbitrary Wi-Fi route can carry the stream.
NVIDIA users should use [Vibepollo](https://github.com/Nonary/Vibepollo).

**Wire-time estimate, not a network measurement:** at 2×40 Mbps pacing and 20%
FEC, ignoring packet headers and link overhead, the measured HEVC 4K40 maximum
occupies about 31.9 ms of packet transmission uncapped, 10.8 ms with a
one-frame cap and 20.0 ms with a two-frame cap (`bytes × 8 × 1.2 /
80,000,000`). That explains the potential to reduce burst pressure. The paced
submission-to-output results check the encoder's contribution to latency;
capture, networking, decoding and presentation remain outside this probe.

Verification: 368 recorded runs completed (60 development screening runs
and 308 release runs). Every submitted frame returned, with one latency
sample per completed frame. Five original matrix runs overlapped a live
stream; they are marked above and both affected mode groups were repeated.
Two of those overlapping runs missed the requested cadence. Every clean
run reached 481 submissions at 60 fps or 961 at 120 fps. No other
`performance` process was observed at the pre-run checks.

The release performance example built successfully. Final checks passed:
`cargo fmt --all`; Clippy for `butterpollo-core` and `butterpollo-windows`,
all targets with `-D warnings`; 166 core tests; and 61 host binary tests.
The host's two existing ignored tests remain ignored (all-interface
listener/firewall interaction and official installer download). `npm ci`
and `npm run check` passed with 0 errors and 0 warnings across 163 files.
Policy tests cover driver-default preservation, codec property names,
bit units at fractional frame rates, and invalid settings. All Cargo
commands used the supplied environment and `--target-dir target\qa`
where applicable.

## October 7: PyroWave release picture checks

On the AMD Radeon RX 7900 XT (driver 32.0.31041.1004), the release receiver now
decodes PyroWave with SDK bitstream `186f0393` and uses the same pixel barcode,
motion, cadence and Opus tone checks as the other codecs.
`build-pyrowave-client.ps1` builds `moonlight_client.c` against the existing
pinned Nonary transport. That transport negotiates records and decrypts video
and audio; the receiver rejects codec fallback, a different bitstream, missing
encryption, lost buffers, invalid records and incomplete SDK frames. Every
delivered picture must decode. The former separate SDK receiver is folded into
the common fixture, so its decoded pixels now supply the release measurements.

The shared barcode reader now accounts for the encoder's letterbox and
measures QPC picture age when the explicit host address is loopback. The
PyroWave gate requires age samples for at least 95% of steady pictures and
records their mean, p95, p99 and maximum. Age ends after SDK decode and CPU
readback, before any client display. Existing limits remain: 97–103% of the
requested cadence, at least 90% fresh pictures, 95% barcode coverage, no
decode failures, bounded delivery gaps, and a continuous audible test tone.

`release.ps1` adds required 1920×1080 at 60 fps runs for SDR 4:2:0 and HDR
4:4:4, each requesting 400 Mbps for 12 seconds with a three-second warmup.
`finalize.py` requires both results. They use the existing retry once and
fail-release path. Only the isolated PyroWave profile enables PyroWave and
requires LAN encryption; no installed configuration is changed. The first
connection checks rejected the default profile because it disabled video
encryption, despite the client's request to encrypt everything.

The live checks used the installed rc.20 executable as a portable host:
source `f06e72c73c1960665d031d27f8dad02202bca56e`, SHA-256
`07b0487e1d9d7312da58fe3430c6d53d0e25e949632284896a94332cc99bc074`.
Its own profile and ports were under `target/pyrowave-e2e`; the running
service was not stopped, restarted, reinstalled or reconfigured. Before
each run, the installed log's last stream event was `CLIENT DISCONNECTED`
at 15:51:34 UTC. The source remained a 5120×1440 SDR desktop, with the
fixture's 120 Hz bottom strip and virtual-speaker tone. The host letterboxed
it into 1920×1080. HDR therefore checks conversion and HDR/4:4:4 negotiation,
not native HDR capture. The SDK CPU output is eight-bit even for HDR;
full-precision HDR quality remains covered by the separate GPU harness above.

SDR and HDR alternated twice. All four runs passed, with 100% barcode and
age coverage, zero repeated steady pictures, zero partial pictures, zero
decode failures, and continuous tone:

| Measurement | SDR runs | HDR 4:4:4 runs |
|---|---:|---:|
| Pictures decoded / delivered | 707/707, 700/700 | 711/711, 704/704 |
| Fresh pictures/s after warmup | 59.994, 60.054 | 60.462, 60.292 |
| Picture age mean, ms | 21.332, 26.025 | 22.787, 24.186 |
| Picture age p95, ms | 22.192, 29.541 | 26.653, 28.707 |
| Picture age maximum, ms | 29.648, 39.370 | 28.330, 32.674 |
| Arrival interval p99, ms | 17.977, 18.005 | 18.650, 18.871 |
| Arrival interval maximum, ms | 19.067, 18.534 | 19.522, 23.357 |

These are two-run spreads on a shared machine, not isolated latency
comparisons. Artifacts are `target/pyrowave-e2e/batch{2,3}/e2e-pyrowave*`;
`batch1` retains the rejected unencrypted connections. Reproduce each case
after loading the Rust environment and building the receiver and the two
probes beside it:

```powershell
python rust/release/e2e.py --package 'C:\Program Files\ButterpolloRust' `
  --work "$PWD/target/pyrowave-e2e/repeat" `
  --client "$PWD/target/pyrowave-e2e/fixtures/moonlight-pyrowave-client.exe" `
  --codec pyrowave --mode 1920x1080x60 --seconds 12 --bitrate 400000
# Repeat with --codec pyrowave-hdr-444. Use the release Python environment.
```

Two additional 12-second H.264/HEVC runs at 2560×720 decoded all 874/864
delivered pictures, with zero decode errors and 100% barcode coverage, but
failed the unchanged audio-continuity gate: minimum tone RMS fell to 0.000013
and 0.000003 respectively. Aggregate live command time, including the early
rejected connections and host startup/cleanup, was about 171 seconds. AV1 and
HEVC VRR were not rerun within that budget.

Checks passed: 12 release-gate unit tests, both C receiver builds (warnings
denied, except the shared fixture's unused callback parameters and compact
indentation), Python compilation, PowerShell parsing, `cargo fmt --all`, clippy
for core/Windows/host with all targets and warnings denied, 167 core tests, and
63 host tests with two existing ignored tests. The host and both probe examples
also built. The ordinary receiver rejects `pyrowave` before connecting instead
of silently requesting H.264.

## October 7: Release audio continuity under CPU load

On the AMD Radeon RX 7900 XT (driver `32.0.31041.1004`) and Ryzen 7
5800X3D (8 cores, 16 logical processors), CPU load reproduced the release
audio failure at the test-tone renderer: it ran out of queued samples.
Giving that fixture the existing media-worker
scheduling guard removed the observed underruns; the audio acceptance
thresholds are unchanged.

These runs used the installed rc.20 `butterpollo.exe`, SHA-256
`07b0487e1d9d7312da58fe3430c6d53d0e25e949632284896a94332cc99bc074`.
`e2e.py` always launches a separate host, so this tested that executable
with the release script's isolated profile and ports, not the running
service's configuration. The service was not stopped, restarted, paired,
reinstalled or reconfigured. Before every run its last stream event was
`CLIENT DISCONNECTED` at 15:51:34 UTC; the script also checked its idle
server state. The original receiver and probes came from
`C:\src\butterpollo-release\review-fixes\qa\fixtures`.

Each stream requested H.264 or HEVC at 2560×720/60, 20,000 kbps and 30
seconds, as the standard release cases do on this 5120×1440 SDR desktop.
The receiver's repeated sleeps lengthened some CPU-loaded runs beyond 30
seconds; cadence uses its measured steady interval. No-added-load runs
were not an idle-machine baseline: other worktrees were compiling. Their
sampled whole-run CPU means ranged from 41.1% to 82.5% (the first pair did
not have utilization sampling).

CPU load used 16 busy processes at default priority. The first two CPU cases
started the workers before host initialization, with a 100-second worker
limit; the H.264 case also overlapped this worktree's first probe build.
The subsequent old/new/new/old comparisons started the workers after the
host capability probe and kept them running through teardown. Those
matched CPU samples were 99.9–100%. Each codec had two old and two fixed
renderer runs, with the order reversed for the second pair.

GPU-only cases alternated with no-added-load cases and ran `gpu_load 45 1000 0
200`. The load probe completed at 162.2–167.9 FPS across the four runs, with
frame-time p95 of 6.883–8.431 ms. The combined cases started the same shader
workload alongside the CPU workers after host initialization and stopped only
these owned processes afterwards. CPU contention also starved the GPU load
process: one combined-case sample showed only 10% 3D-engine utilization for
that process.

Counts below are **pass / fail**. Audio and the complete e2e gate are
separate; every completed stream decoded every delivered picture with
zero decode errors. "Original" uses the supplied fixtures; "fixed" changes
only `audio_probe.exe`, keeping the same receiver, motion probe and host.

| Fixture | Added load | H.264 audio | HEVC audio | H.264 complete e2e | HEVC complete e2e |
| --- | --- | ---: | ---: | ---: | ---: |
| Original | None | 3 / 0 | 3 / 0 | 3 / 0 | 3 / 0 |
| Fixed | None | 1 / 0 | 1 / 0 | 1 / 0 | 1 / 0 |
| Original | CPU | 0 / 3 | 0 / 3 | 0 / 3 | 0 / 3 |
| Fixed | CPU | 2 / 0 | 2 / 0 | 1 / 1 | 2 / 0 |
| Original | GPU | 1 / 0 | 1 / 0 | 0 / 1 | 0 / 1 |
| Fixed | GPU | 1 / 0 | 1 / 0 | 0 / 1 | 0 / 1 |
| Original | CPU + GPU probe | 1 / 0 | 0 / 1 | 1 / 0 | 0 / 1 |
| Fixed | CPU + GPU probe | 1 / 0 | 1 / 0 | 0 / 1 | 0 / 1 |

In the four matched CPU comparisons, original minimum tone RMS was
0.000001–0.000007; fixed minimum RMS was 0.008942 in every run. All ten
fixed-renderer runs had continuous audio and zero source underruns. Five
still failed video cadence or fresh-picture requirements under load;
those failures remain failures. The GPU-only steady picture rates ranged
from 62.777 to 66.831 FPS and fresh-picture rates from 42.074 to 50.048 FPS,
outside the existing release limits despite successful decoding.

A separate instrumented H.264 CPU run recorded the renderer's padding
and every received audio packet. Excluding the initial empty buffer,
the 4,800-frame / 100 ms render buffer ran dry 18 times. Nine groups of
quiet decoded blocks occurred 3.72–34.67 seconds into decoded audio;
each group's start was within 18.3 ms of an empty-buffer refill. The
29 affected packets were ordinary encoded packets, not concealment:
all 8,011 decoded packets contained 240 frames (5 ms), and receiver
statistics recorded zero failed FEC recovery, recovered packets,
out-of-sequence packets or PLC calls. There were no host
`audio lost on the host before sending` or UDP-send warnings. This
places the reproduced interruption at the tone source, before host
capture, rather than in the send path, loopback transport or decoder.
Both instrumented no-added-load runs passed. A diagnostic HEVC CPU
attempt timed out at host initialization before streaming and is not
counted as an audio result in the table.

The low-energy blocks were real interruptions, not borderline threshold
or startup failures: the receiver already skips two seconds of decoded
audio, and its 2.5 ms windows still require at least half the strongest
window's RMS. A private negative-test renderer deliberately slept for
200 ms eight seconds after starting. The unchanged gate rejected it,
with minimum RMS 0.000004 and one source underrun, while all 1,780 video
pictures decoded. No startup exclusion or dropout tolerance was widened.

The probe now uses `Priority::new()` as the host's media workers do and logs
`AUDIO_RENDER_UNDERRUN` when its queue empties after initial filling. `e2e.py`
includes the source log in evaluation. Results record the source underrun count
(unknown for old probes), and an interrupted tone with source underruns retains
its failure and adds that evidence. The release script prints the failure
reasons before its existing single retry, preserves the first attempt, and
still stops on a second failure.

Loopback is explicitly recognized by `peer.ip().to_canonical().is_loopback()`
and keeps the 800 Mbps pacing ceiling; these tests did not exercise the
then-current unknown-route 2× default. The host audio, send-path and pacing
sources are unchanged between rc.20 source `f06e72c7` and this worktree's
starting `7908fb4a`. No host changes were made here. These measurements do not
implicate QoS, control ACK holding or the optional AMF limits in the reproduced
failure.

Raw results, CPU samples, commands and temporary diagnostic sources are
in this worktree's `target/audio-e2e`. `baseline-*`, `compare-*` and
`fixed-*` contain the table's runs; `trace-*` and `negative-dropout`
contain the diagnostic and deliberate-failure cases. `batch.py` and
`compare.py` record the installed log event before each case and clean
up only their own load processes. `measurements.json` collects the
receiver results. The new probe is built by the existing release script;
manual runs can copy the receiver and motion probe into a private
fixture directory and replace only its audio probe with the executable
built by `cargo build --release -p butterpollo-windows --example
audio_probe --target-dir target\qa` after loading `rust-env.ps1`.

Validation passed: `cargo fmt --all`; Windows clippy with all targets and
warnings denied; 169 core tests; 63 host tests with two existing ignores; and
14 release-gate unit tests. The release audio/GPU probes built. A PowerShell
syntax check and a stubbed execution of the actual retry loop verified that the
first failure is retained and a second failure stops the release. The
deliberate source-pause e2e failed as expected.

## October 7: 116 FPS VRR capture, WGC and DDX

This checks the report of an RX 9070 XT alternating between about 116 and
60–95 FPS in five-second stream windows. The available machine is an AMD
Radeon RX 7900 XT, driver `32.0.31041.1004`, Windows `26200.9550`, with the
Sunshine virtual display driver `1.6.3.0`. It is not a reproduction on the
reporter's GPU or game.

Both WGC and DDX delivered every distinct picture in capture-only runs whose
VRR source held 116 or 120 FPS, including 4K HDR. The 1080p stream comparisons
also sustained about 116 FPS with front-edge and async RTSS pacing.
Simultaneous-work runs and the local receiver had substantial slowdowns,
described separately below.

The release host was built from `dac86ada` in this worktree. The stream
tests use an isolated SYSTEM host on localhost port 48923, a private
configuration, per-client virtual displays, HEVC HDR, compute conversion
and the normal AMF defaults. The initial runs use an exclusive layout;
the later RTSS and receive-only runs use an extended layout so another
task's physical-display soak can continue. WGC uses the signed-in user's
helper. The source is `motion_probe`, a borderless full-display D3D11
window with a frame number and QPC timestamp in its pixels. Its successful
`Present(1)` calls are recorded independently of capture. A receiver built from
`tests/moonlight_client.c` uses D3D11VA and reads those numbers after decode.
Receiver work therefore shares this GPU; this is not a remote-client or
physical VRR scanout test.

The capture, frame-generation policy, display-session, display and limiter
sources match rc.20 (`f06e72c7`); the intervening `stream.rs` changes do not
alter capture or VRR policy. This is not a comparison between two host
versions with different capture defaults.

The stream fixture sets `clientVrrRequested=1`; RTSP carries that launch
request into `vrr_low_latency`, and the host log confirms `vrr=true`.
With the default `frame_limiter_auto_virtual_framegen=legacy`, the policy
requests 1000 Hz when VRR is requested and twice the stream rate otherwise
(232 or 240 Hz here). Turning virtual display refresh off bypasses that
automatic 1000 Hz policy. A manual display refresh or device display-mode
override also needs to be checked when diagnosing a different machine.
`kept_timings=true` in the activation message means the *other displays*
kept their timings while Windows switched on the new target. It is not a
measurement of the new display's refresh.

RTSS `SyncLimiter=1` is front-edge sync, `0` is async and `2` is back-edge
sync. The installed RTSS help at
`Help/Properties/General/SYNC_LIMITER` describes front-edge sync as pacing
the start of the presentation call against the system clock. Scanline sync
is a separate setting; front-edge sync does not itself request half rate.
The virtual-display policy normally selects front-edge sync on AMD even
when the configured RTSS mode is async. To test async, enable **Use this
mode on virtual displays** (`rtss_allow_virtual_display_override=true`)
as well as choosing **RTSS limit mode → Async**. Game-provided frame
generation has its own override policy in `core/src/framegen.rs`.

RTSS was copied to the private artifact directory, with global hooking
disabled and only the uniquely named `vrr_rtss_source.exe` opted in. The
source runs as the signed-in user without its own rate cap; its module
list confirms the private `RTSSHooks64.dll` is loaded. An initial run with
only global RTSS settings presented 239.30 FPS despite the host verifying
`Limit=116`. A later run with the private application profile set to 116
gave 115.99 source FPS and all 2,320 measured pictures through WGC in
20 seconds. This is why the host's verified global limit is insufficient
evidence of the game's effective limit: check its application profile and
actual presentation rate too. Subsequent limiter comparisons set both
the global lease and that private application profile to the same rate
and mode. No installed RTSS profile is edited.

An additional attempt to force 60 FPS through that private application's
profile did not establish a 60 FPS source: it presented 140.93 FPS in the
WGC run and 136.06 FPS in the DDX run. The hook was absent from the WGC
module snapshot and present in the DDX snapshot. Both failed the motion
gate under simultaneous work. These are failed limiter controls, not
evidence that an application profile caused the reported 60 FPS problem.

The rc.20 source already sets WGC's `MinUpdateInterval` to zero, uses a
two-frame pool, and disables slot-aligned publication for VRR streams.
The legacy diagnostic `wgc_high_rate_capture=true` requests 1 ms instead.
Those settings should not be described as a new fix for this report.

Capture-only measurements use `capture_phase_probe`: ten seconds per
run, excluding the first second and last 250 ms, in WGC-helper / DDX /
DDX / WGC-helper order. The eight-pixel source changes a sequence number
at the requested rate; capture reads it only after the full desktop copy
finishes. This counts distinct captured pictures, without an encoder or
receiver. The temporary `vrr_hdr_probe` variant uses the production
display guard to enable HDR on its own virtual display and restores that
lease afterwards. Its capture textures were float16 (DXGI format 10),
versus BGRA8 (87) in the original probe. Its source and preparation script
remain with the raw artifacts, not in the shipped examples.

In the main capture-only matrix, 38 of 48 runs held their requested rate
within 0.1 FPS. Those runs captured all 39,202 presented pictures. The
table combines WGC and DDX counts; each rate normally has two runs per
backend. Runs whose source slowed are included in the denominator, and
described below, rather than being counted as successful fixed-rate tests.

| Desktop | Format | Measured refresh | Requested source FPS | Runs holding target / total | Pictures captured / presented in those runs |
| --- | --- | --- | --- | --- | --- |
| 1080p | SDR | 1000 Hz | 116 and 120 | 8 / 8 | 8260 / 8260 |
| 4K | SDR | 1000 Hz | 116 and 120 | 8 / 8 | 8261 / 8261 |
| 1080p | SDR | 232 Hz | 116 | 1 / 4 | 1015 / 1015 |
| 1080p | SDR | 240 Hz | 120 | 0 / 4 | no fixed-rate result |
| 4K | SDR | 232 Hz | 116 | 4 / 4 | 4061 / 4061 |
| 4K | SDR | 240 Hz | 120 | 4 / 4 | 4200 / 4200 |
| 1080p | HDR | 1000 Hz | 116 and 120 | 7 / 8 | 7245 / 7245 |
| 4K | HDR | 1000 Hz | 116 and 120 | 6 / 8 | 6160 / 6160 |

Repeating the incomplete controls in the same alternating order gave
four successful runs each at 1080p SDR / 232 Hz / 116 FPS, 1080p SDR /
240 Hz / 120 FPS, and 4K HDR / 1000 Hz / 120 FPS. All 12 held the target
and captured all 12,460 presented pictures. Across the main matrix and
these repeats, the 50 runs that held their rate captured 51,662 of
51,662 pictures; the ten earlier source-slowdown runs remain recorded.

The initial 1080p HEVC HDR streams below use the source's own cap and no
host limiter. Each row is one run. Source and distinct decoded pictures
are counted over the same 20-second source window; host FPS is the
fixture's steady send counter and can include repeated pictures.

| Requested FPS | VRR / measured Hz | Capture | Source FPS | Host FPS | Distinct decoded FPS |
| --- | --- | --- | --- | --- | --- |
| 116 | on / 1000 | WGC helper | 116.00 | 116.83 | 115.25 |
| 116 | on / 1000 | DDX | 116.00 | 116.88 | 114.75 |
| 116 | off / 232 | WGC helper | 115.42 | 116.71 | 113.00 |
| 116 | off / 232 | DDX | 115.33 | 116.74 | 111.25 |
| 120 | on / 1000 | WGC helper | 119.40 | 120.76 | 118.40 |
| 120 | on / 1000 | DDX | 120.00 | 120.90 | 118.80 |
| 120 | off / 240 | WGC helper | 120.00 | 120.81 | 118.40 |
| 120 | off / 240 | DDX | 118.80 | 117.61 | 112.55 |

All eight passed the fixture's interoperability and motion checks.
Distinct-picture coverage was 94.74–99.35%, so these are not claims of
lossless end-to-end delivery. Neither backend fell to a sustained half
rate. In the 116 FPS VRR pair, four steady five-second windows were
116.80–117.02 FPS with WGC and 116.79–116.95 FPS with DDX; send-interval
p95 was 9.08–9.13 ms and 9.09–9.17 ms respectively. With RTSS pacing the
uncapped 1080p source at 116 FPS and VRR on, both modes delivered every
picture in the common 20-second window:

| RTSS mode | Capture | Source FPS | Distinct decoded FPS | Source pictures received |
| --- | --- | --- | --- | --- |
| Front-edge | WGC helper | 116.00 | 116.00 | 2320 / 2320 |
| Front-edge | DDX | 115.65 | 115.65 | 2313 / 2313 |
| Async | DDX | 115.56 | 115.60 | 2312 / 2312 |
| Async | WGC helper | 115.85 | 115.85 | 2317 / 2317 |

The small difference between an interval-derived source FPS and pictures
divided by 20 seconds is rounding and the window boundary. These runs do
not support replacing front-edge sync with async as a general fix.

At 4K with VRR and front-edge sync, WGC had 116.00 source FPS and 116.05
assembled frames per second; DDX had 114.40 and 114.50. These use the
receive-only client described below and the common 20-second window.
The 4K async DDX run slowed to 92.84 source FPS and received 95.25 frames
per second; the async WGC case hit the outer launcher timeout. The 4K
async comparison is therefore inconclusive.

Several simultaneous-work runs failed to maintain the requested source rate.
For example, the last two 4K HDR / 120 FPS runs presented 76.03 and 59.54 FPS,
while DDX captured 72.04 and WGC captured 51.98 respectively. The first 1080p
HDR / 116 run presented 80.83 and captured 77.74 with WGC. The 1080p SDR / 240
Hz batch fell as low as 38.92 source FPS and 29.68 captured FPS. These are
retained observations, not clean comparisons of the backends. The soak suite
and other builds were active during this part of the study; their individual
contributions were not isolated. A small capture age or a lower game resolution
alone would not distinguish this situation from source-side pacing or
scheduling problems.

The first full 4K receiver comparison was also unsuitable for measuring
capture losses. Its hardware-decoder callback includes GPU-to-CPU
readback and took 33.70 ms on average in the WGC run and 14.32 ms in the
DDX run. Received rates were 28.35 and 67.98 FPS, while the host's steady
send counters were 106.71 and 116.75. The WGC case failed the motion gate
and had repeated keyframe requests; the DDX case passed interoperability
but did not sustain the requested rate. Neither is a valid end-to-end
4K/116 performance pass. Subsequent 4K stream runs use a private receiver
that counts assembled frames without decoding; those measurements must
not be labelled decoded or fresh-picture FPS.

The original fixture also waited for a fixed number of 100 ms sleeps.
Under simultaneous work, a nominal 32-second receiver run lasted 54.36
seconds, outliving the 36-second source. Its 69.86 host FPS average is
misleading: the common 20-second window had 109.05 source FPS and 108.50
assembled frames per second. The later private receiver uses a wall-clock
deadline. Two early cases exceeded the SYSTEM launcher's 120-second
deadline, and another receiver timed out; their partial logs remain in
the artifacts. Later launches use the existing `session_command_long`
example with a 300-second outer deadline.

The final 4K matrix reduced `motion_probe` to a 128-pixel-high window to
reduce its drawing work. The desktop, capture and encoded output remain
3840×2160 HDR. This uses the source's own cap, no host limiter, the
wall-clock receiver and the same 20-second source/receive window:

| Requested FPS | VRR / measured Hz | Capture | Source FPS | Assembled frames per second |
| --- | --- | --- | --- | --- |
| 116 | on / 1000 | WGC helper | 96.34 | 98.65 |
| 116 | on / 1000 | DDX | 95.53 | 98.20 |
| 116 | off / 232 | WGC helper | 104.91 | 104.70 |
| 116 | off / 232 | DDX | 105.20 | 104.70 |
| 120 | on / 1000 | WGC helper | 119.24 | 119.25 |
| 120 | on / 1000 | DDX | 120.00 | 120.00 |
| 120 | off / 240 | WGC helper | 119.95 | 119.60 |
| 120 | off / 240 | DDX | 100.27 | 100.55 |

All eight completed the receive-only interoperability check. The source
still failed to hold its cap in several rows, so this is not a clean
ranking of the capture backends. Assembled-frame counts can include
repeats and other desktop updates; they cannot establish fresh-picture
coverage. The capture-only sequence-number measurements above answer
that narrower question. A quiet-machine 4K encode/decode comparison and
the original RX 9070 XT/game reproduction remain outstanding.

No capture or limiter policy change is justified by these measurements. In
particular, they do not support automatically preferring DDX for VRR, changing
the WGC frame pool or dirty-region handling, or changing the default RTSS sync
mode.

For that report, try **Capture method → Desktop Duplication** as a single
comparison, with the same game scene, client settings and verified source
rate. Check the in-game FPS overlay both before streaming and during the
slow stream windows. If the game itself is near 60, inspect its VSync and
frame-cap settings, the selected virtual display's actual refresh, and
its RTSS application profile. A useful limiter control is **Limiter →
None** under Frame limiter, with RTSS disabled for that application and an
in-game 116 FPS cap. Alternatively, test Async with the virtual-display
override described above. Async is a diagnostic comparison, not a
demonstrated fix.

Raw commands, isolated profiles, source timestamps, received picture IDs,
host five-second windows and connection checks are in
`target/vrr-capture`; `measurements.json` and `phase-measurements.json`
collect the counts. The SYSTEM launcher uses `session_command.exe` from
the supplied `bench-rc17` harness and, for later batches, the existing
`session_command_long` example built in this worktree. It does not change
the installed service's files, settings or lifetime. Stream cases check the installed log
and public session counts before starting and watch them during the run;
capture-only batches record the latest connection event and process list.
The installed service stayed idle, with its last connection event the
15:51:34 UTC disconnect. The other RX 9070 investigation overlapped the
initial capture pilot; the soak task and other builds overlapped later
batches. Process snapshots are retained beside the measurements.
Test-owned processes exited and their scheduled tasks were removed after
the runs. The installed RTSS global-profile SHA-256 is unchanged.

Validation passed: `cargo fmt --all`; clippy for `butterpollo-core`,
`butterpollo-windows` and `butterpollo`, with all targets and warnings
denied; 171 core tests; and 63 host tests, with two existing ignores.
The release host and capture probes built successfully. This section is
the only tracked change; the experimental harness and receiver remain
under `target/vrr-capture`.

## October 12: single-pass D3D11 4:2:0 conversion

The D3D11 fallback converts NV12/P010 and PyroWave 4:2:0 with one compute
dispatch on the existing immediate context, reusing the existing shaders.
Unsupported shader or unordered-access-view creation retains the pixel-pass
fallback. The separate D3D12 compute queue and PyroWave 4:4:4 MRT draw remain
unchanged. This applies to the fallback, including
`gpu_compute_conversion=false`; the default AMD D3D12 path is unchanged.

Release-build D3D11 timestamps on the RX 7900 XT compared the previous pixel
passes with the single dispatch in ABBA order: two runs per path, 32 warmups
and 128 samples per run. Outputs were 1920×1080, 2560×1440 and 3840×2160,
using a 3840×2160 BGRA8 SDR source or FP16 HDR source. The loaded comparison
used `gpu_load 20 1000 60 200`. These GPU conversion times exclude capture,
encoding, transport, decoding and scanout.

| 3840×2160 output | Idle previous / single pass, mean ms | Loaded previous / single pass, mean ms |
| --- | ---: | ---: |
| NV12 SDR | 0.2348 / 0.1230 | 0.3159 / 0.2018 |
| P010 HDR | 0.2300 / 0.1450 | 0.3125 / 0.2157 |
| PyroWave SDR 4:2:0 | 0.3060 / 0.1032 | 0.4667 / 0.1427 |
| PyroWave HDR 4:2:0 | 0.3065 / 0.1237 | 0.4771 / 0.1619 |

PyroWave mean GPU time fell 32–68% idle and 33–69% under this load across
the three resolutions. NV12/P010 improvements were strongest at 1440p and
4K. Loaded 1080p video results were less decisive: NV12 mean changed from
0.3055 to 0.2990 ms, while p95 changed from 0.2446 to 0.2482 ms; P010 p95
changed from 0.2686 to 0.2764 ms. The 4K case avoids resampling the 4K source,
so its times do not establish scaling cost from resolution alone. CPU call
times were mixed. These measurements cover only this Radeon GPU.

All nine native GPU correctness tests passed. Exact comparisons covered 480
NV12/P010 cases and 480 PyroWave planar cases, including every luma/chroma
byte, SDR eight/ten-bit, FP16 SDR/HDR, matrix/range settings, scaling,
letterboxing, dimensions that do not divide evenly into thread groups, and
monochrome/clipped/alpha cursors. The D3D11 debug layer accepted both video
plane UAV bindings without warnings or errors. Existing D3D12/pixel
comparisons, 4:4:4 output, HDR colour and codec-held texture tests also passed.

Isolated 1080p60 streams with `gpu_compute_conversion=false` passed the AMF
HEVC SDR/HDR and PyroWave SDR 4:2:0 gates: respectively 686/686, 715/715 and
676/676 pictures decoded, with continuous audio and full motion coverage.
WGC captured a 5120×1440 SDR desktop, so the HDR streams check conversion and
negotiation rather than native HDR capture. The first PyroWave HDR run
overlapped unit tests and failed the delivery-gap limit with nine long
intervals, despite decoding all 716 pictures. With other jobs stopped,
20-second repeats passed for both the previous build (1176/1176 decoded,
60.012 fresh fps, three long intervals) and the candidate (1199/1199,
59.982 fresh fps, four long intervals), with no decode errors or audio
underruns. The failed first run remains recorded. These checks establish
interop and correctness; no CPU savings or end-to-end latency improvement
is claimed.

After removing the pacing experiments, the final GPU-only 2.2.8 build passed
four quiet 20-second loopback runs at 1080p60/40 Mbps with the same fallback
configuration. HEVC SDR/HDR and PyroWave SDR/HDR decoded respectively
1198/1198, 1198/1198, 1181/1181 and 1199/1199 pictures, at 59.994–60.002
steady fps. Audio was continuous, with no decode errors; long intervals
were respectively zero, zero, zero and one. Evidence is under
`work/gpu-only-e2e`. The [pacing follow-up](PERFORMANCE_WORK.md#october-12-420-and-pacing-follow-up)
records the unsuccessful combined-candidate Wi-Fi comparison and a separate
completed GPU-only laptop check. Four ABBA runs against 2.2.7 used official
Moonlight 6.2.0 on a Radeon 780M over WLAN, at 1080p60 HEVC SDR with the
same fallback configuration. Each ran for approximately 165 seconds, with
120-second requested host measurement windows. All four reported 59.99
rendered fps, 0.00% network drops and 0.03% jitter drops; the host windows
had no sender drops, IDR requests or reference invalidations. The repeated
baseline matched candidate send p95. This short check found no observed
regression at that workload; rounded client statistics and absent FEC reports
do not establish exact packet loss, and no end-to-end latency improvement
is claimed. The final GPU-only build contains no pacing changes.

Reproduce with the ignored release tests in `gpu_color::tests`:
`gpu_420_single_pass_timing`, `gpu_420_single_pass_matches_pixel_passes` and
`gpu_420_video_plane_uavs_are_disjoint`. The timing test retains the old pixel
passes as its reference and writes JSON to `BUTTERPOLLO_TEST_GPU_REPORT`.
Run timing separately from builds and other workloads, except the explicit
synthetic GPU load. Local evidence: `gpu-420-timing-idle.json`,
`gpu-420-timing-loaded.json`, `gpu-regressions.log`, and the `candidate-e2e`,
`baseline-e2e-quiet` and `candidate-e2e-quiet` stream results.

## October 12: Rubylight 2.2.9 released-package refresh

**The physical-SDR local fixtures, real Wi-Fi receiver matrix, native HDR
headline/pixel measurements, stock Moonlight checks and retained AMF
settings sweeps have completed. Their individual outcomes and limitations
remain separate; completion does not mean every case passed.**
The portable repeat-frame matrix passed all 81
cases on October 12, 2026, from 06:15 to 06:28 CEST. CPU, packet, input,
wait and native GPU component probes also completed, alongside 16/16
PyroWave compute/load cases, 16/16 synthetic-motion cases, 32/32 AMF quality
cases and 4/4 independent UDP pacing checks.
The [retained AMF property sweeps](#retained-amf-property-sweeps-october-12)
completed **1,030 cases: 1,018 passed and 12 failed**, with every original
failure retained.

The [current stream record](PERFORMANCE_WORK.md#rubylight-229-short-release-streams-2026-10-12)
retains every outcome: 36/36 short release streams, 12/12 controls, 8/8
compute checks, 4/4 default-pacing streams and 12/12 static-IDR checks passed.
The supplemental stage passed 10/12 checks; both 4K120 HEVC streams and both
240 FPS saturation attempts failed. The audio/load/80 ms outage batch passed
6/11 full-stream checks, with continuous audio in all 11. All four 90-second
controls passed; all four deliberate 20 ms outage streams failed the normal
gap gate. Both 120-second audio/video streams passed. The corrected short soak
completed 67 cases: 64 passed, three failed GPU-load freshness; all 52 churn
cycles, eight codec switches, capture-helper recovery and host working-set,
handle and thread growth checks passed. The [GPU-memory follow-up](PERFORMANCE_WORK.md#gpu-memory-counter-follow-up-october-12)
reproduced the rising process counter as a reporting artifact: all 30 HEVC
reconnects passed, and adapter-wide dedicated usage returned to baseline
after idle. No sustained VRAM growth was observed in that focused test.
The [soak record](PERFORMANCE_WORK.md#short-soak-switching-reconnects-and-wgc-helper-recovery)
retains the original counter readings separately from adapter consumption.
The initial soak attempt could not start its direct receivers
because of a missing DLL search path and is retained as invalid fixture
evidence, not a set of product failures.

The [real Radeon 780M Wi-Fi matrix](PERFORMANCE_WORK.md#real-wi-fi-hardware-receiver-streams-october-12)
passed **20/20 ordinary receiver checks**: 12 HDR-output conversion runs and
eight SDR control/recovery runs, including four injected-outage cases.
**19/20 calibrated timing records** passed exact source validation; AV1 HDR
1080p60 run 2 remains unavailable because three decoded barcode timestamps
did not match the source ledger. The hardware receiver's arrival boundary
is frame assembly/enqueue, before decoder queueing, decoding and display.
Its 1.177 ms clock-fit diagnostic is not a confidence interval or proven
error bound. These are physical-SDR source measurements, not native HDR.

The [native HDR streams and pixels](#native-virtual-hdr-results-october-12)
passed 28/28 and 4/4 checks respectively. The SDR capture-only sweep attempted
32 runs: 20 matched the requested mode; 12 have unavailable requested-mode
timings because Windows selected different refresh rates. The historical
private HDR capture/RTSS fixture sources were not retained. Current results
do not establish matched deltas against the earlier native-HDR, Wi-Fi or
historical competitor evidence.

### Native virtual-HDR results (October 12)

All **28/28** native headline streams passed the ordinary receiver gates and
exact decoded source ID/QPC validation. The virtual capture source is native
HDR at **120 Hz**, with DDX and WGC verified separately. The active physical
desktop remains **5120x1440 SDR at 240 Hz**; the historical October 8 runs
used a **2560x1440 / 120 Hz** physical desktop. The current host/driver and
all dated competitor results retain their separate provenance; no percentage
improvement or matched before/after conclusion follows from this table.

These use the published host and a pinned independent D3D11VA receiver on
the same RX 7900 XT. Picture age runs from the source QPC timestamp through
decoded-barcode readback, excluding presentation and display scanout. The
separate hardware `arrival_ms` field estimates frame assembly/enqueue,
not decode start; adding callback time to it omits decoder-queue waiting.
This local picture-age endpoint also differs from the Wi-Fi
render-to-received endpoint. Each stream requests 35 seconds with five
seconds excluded for warmup. The loaded fixture is `gpu_load 45 1000 60 200`;
all 12 load processes completed naturally at **60.000 FPS / 2,701 frames**.

Every cell below is the arithmetic mean of **two per-run statistics**,
including the p95 and p99 columns: these are not pooled percentiles.
All age and host columns are milliseconds. The [per-repeat record](PERFORMANCE_WORK.md#native-virtual-hdr-headline-streams-october-12)
retains variation, decoder times, source checks and executable hashes.

| Capture | Codec / output / requested FPS / Mbps / load | Picture age mean / p95 / p99 | Fresh FPS | Host mean |
| --- | --- | ---: | ---: | ---: |
| DDX | HEVC HDR / 1968x2184 / 120 / 80 / idle | 9.649 / 10.868 / 12.165 | 119.993 | 3.296 |
| DDX | AV1 HDR / 1968x2184 / 120 / 80 / idle | 10.502 / 11.256 / 11.610 | 119.998 | 3.032 |
| DDX | HEVC HDR / 2560x1440 / 120 / 50 / idle | 9.385 / 9.892 / 10.194 | 119.999 | 2.970 |
| DDX | HEVC HDR / 1920x1080 / 60 / 20 / idle | 10.092 / 10.557 / 10.763 | 60.000 | 2.139 |
| DDX | HEVC HDR / 1968x2184 / 120 / 80 / load | 14.339 / 16.026 / 18.151 | 119.990 | 3.347 |
| DDX | AV1 HDR / 1968x2184 / 120 / 80 / load | 13.645 / 17.995 / 18.951 | 119.984 | 5.249 |
| DDX | HEVC HDR / 1920x1080 / 60 / 20 / load | 10.638 / 18.265 / 18.580 | 60.002 | 2.237 |
| WGC | HEVC HDR / 1968x2184 / 120 / 80 / idle | 12.238 / 13.734 / 15.910 | 119.983 | 3.262 |
| WGC | AV1 HDR / 1968x2184 / 120 / 80 / idle | 10.572 / 11.775 / 13.520 | 120.008 | 3.037 |
| WGC | HEVC HDR / 2560x1440 / 120 / 50 / idle | 11.291 / 11.996 / 12.203 | 120.001 | 2.972 |
| WGC | HEVC HDR / 1920x1080 / 60 / 20 / idle | 9.359 / 9.927 / 10.228 | 60.001 | 2.273 |
| WGC | HEVC HDR / 1968x2184 / 120 / 80 / load | 16.747 / 19.389 / 19.750 | 119.945 | 3.383 |
| WGC | AV1 HDR / 1968x2184 / 120 / 80 / load | 17.320 / 19.478 / 21.933 | 119.868 | 3.043 |
| WGC | HEVC HDR / 1920x1080 / 60 / 20 / load | 11.938 / 12.517 / 15.487 | 59.999 | 2.371 |

The separate [native HDR pixel roundtrip](PERFORMANCE_WORK.md#native-hdr-pixel-roundtrip-october-12)
passed **4/4** WGC software-decoded HEVC/AV1 checks at 720p60 / 40 Mbps.
This confirms the existing pixel gates, not physical-panel calibration.
The [SDR VRR capture-only record](PERFORMANCE_WORK.md#sdr-vrr-capture-only-measurements-october-12)
retains **20 requested-mode-valid runs out of 32 attempted**. All 20 captured
every retained presented picture. The other 12 requested-mode timings are
unavailable: 1080p240 selected 120 Hz, 4K232 selected 928 Hz, and 4K240
selected 960 Hz. They were not retried or counted as requested-mode passes.
All eight capture groups restored the recorded active-monitor state.
Full raw paths and validation hashes are indexed in `work\native-summary.json`.

### Package, source and scope

- Published release: **2.2.9**, source commit
  `d0bfcb5e764095250b8f0e22fd5a0cb3656d5ea6`.
- Release ZIP: `rubylight-2.2.9-windows-x64.zip`, SHA-256
  `2a9b76a0380ec60e189de040656366302dfce3ee2d84d895720eb84a36b81289`.
- Packaged `butterpollo-performance.exe`, SHA-256
  `078470dac3e1ccfbb33cd83e97d4b74d4735ea054cb891571f4a3a84f1ee4ced`.
- Host: Ryzen 7 5800X3D, 8 cores / 16 threads; Radeon RX 7900 XT,
  AMD driver **32.0.32015.2008**; Windows 11 Pro build 26200.
- Capture source for every portable case: **5120×1440 SDR at 240 Hz**, the physical
  Odyssey G93SC, through WGC. Display mode and HDR state were unchanged.

The probe captures one desktop image per process and repeatedly encodes that
image. The desktop was not frozen across cases, so this is not a matched
content comparison with earlier measurements. **HDR output below is converted
from the SDR source; it is not native HDR capture.** The 4K rows are resized
output from that source, not native 4K capture.

Each row contains three separate runs: unpaced GPU input, unpaced CPU
compatibility input, and GPU input paced at 120 fps. CPU mode reads the captured
image back once before measurement; it does not measure repeated GPU readback
or CPU colour conversion in isolation. AMF remains the video encoder for
H.264, HEVC and AV1. PyroWave uses its own encoder and record framing, with
4:2:0 or 4:4:4 as labelled.

These 81 runs executed serially without added synthetic GPU load. Each requested
eight measurement seconds; recorded durations were 8.000–8.036 seconds after
warmup. There is one run per path and setting, not a mean across repeats.
Capacity is completed encoded frames divided by measured time. The paced
column is submission-to-observed-output **mean / p95 in milliseconds**; those
runs completed 120.09–120.11 fps. These values exclude capture age, transport,
client decoding and display latency. Completed encodes alone do not establish
independent decoding, exact coded dimensions or fresh-picture throughput.

### 1920×1080 output

Requested bitrate: **20,000 kbps** for H.264 / HEVC / AV1 and **400,000 kbps**
for PyroWave. Every path requests 120 fps; the two capacity paths are unpaced.

| Codec / output | GPU capacity, fps | CPU path capacity, fps | GPU paced at 120 fps, mean / p95 ms |
| --- | ---: | ---: | ---: |
| H.264 SDR | 667.00 | 144.95 | 2.224 / 2.399 |
| HEVC SDR | 671.09 | 146.29 | 2.315 / 2.612 |
| HEVC HDR output | 670.63 | 101.14 | 2.252 / 2.598 |
| AV1 SDR | 849.88 | 147.29 | 1.755 / 1.872 |
| AV1 HDR output | 848.43 | 101.17 | 1.900 / 2.028 |
| PyroWave SDR 4:2:0 | 2423.90 | 99.41 | 0.666 / 0.781 |
| PyroWave SDR 4:4:4 | 2270.61 | 114.92 | 0.827 / 0.986 |
| PyroWave HDR output 4:2:0 | 2462.08 | 125.33 | 0.959 / 1.077 |
| PyroWave HDR output 4:4:4 | 2211.53 | 132.70 | 0.985 / 1.025 |

### 2560×1440 output

Requested bitrate: **50,000 kbps** for H.264 / HEVC / AV1 and **600,000 kbps**
for PyroWave. This is not the older 40,000 kbps HEVC fixture.

| Codec / output | GPU capacity, fps | CPU path capacity, fps | GPU paced at 120 fps, mean / p95 ms |
| --- | ---: | ---: | ---: |
| H.264 SDR | 419.06 | 133.73 | 3.174 / 3.597 |
| HEVC SDR | 415.95 | 134.13 | 3.354 / 3.641 |
| HEVC HDR output | 415.75 | 60.31 | 3.425 / 3.709 |
| AV1 SDR | 521.62 | 133.17 | 2.688 / 3.069 |
| AV1 HDR output | 520.86 | 60.91 | 2.709 / 3.083 |
| PyroWave SDR 4:2:0 | 2066.73 | 134.83 | 1.209 / 1.355 |
| PyroWave SDR 4:4:4 | 1722.53 | 136.84 | 1.012 / 1.210 |
| PyroWave HDR output 4:2:0 | 2054.49 | 138.41 | 1.006 / 1.106 |
| PyroWave HDR output 4:4:4 | 1668.18 | 138.62 | 1.027 / 1.224 |

### 3840×2160 output

Requested bitrate: **80,000 kbps** for H.264 / HEVC / AV1 and **800,000 kbps**
for PyroWave.

| Codec / output | GPU capacity, fps | CPU path capacity, fps | GPU paced at 120 fps, mean / p95 ms |
| --- | ---: | ---: | ---: |
| H.264 SDR | 199.48 | 71.76 | 5.954 / 6.166 |
| HEVC SDR | 200.48 | 69.51 | 5.876 / 6.167 |
| HEVC HDR output | 200.82 | 27.63 | 5.788 / 6.147 |
| AV1 SDR | 250.00 | 71.83 | 5.106 / 5.364 |
| AV1 HDR output | 249.25 | 26.80 | 4.851 / 5.166 |
| PyroWave SDR 4:2:0 | 1660.24 | 136.91 | 1.106 / 1.264 |
| PyroWave SDR 4:4:4 | 1114.62 | 136.55 | 1.368 / 1.566 |
| PyroWave HDR output 4:2:0 | 1603.32 | 138.60 | 1.096 / 1.230 |
| PyroWave HDR output 4:4:4 | 1028.88 | 137.21 | 1.402 / 1.521 |

### PyroWave compute conversion with and without GPU load

All **16/16** repeat-frame cases passed, from 07:16 to 07:24 CEST on October
12. They use the same package and physical **5120×1440 SDR / 240 Hz** WGC
source, converted to **1920×1080 HDR 4:4:4 output at 120 fps and 400,000 kbps**,
with PyroWave record framing. Each process captures its own repeated image;
desktop content was not frozen across cases. These are SDR-source conversion
results, not native-HDR capture or a matched comparison with the older fixture.
Capture age, transport, decoding and display latency are excluded.

`gpu_compute_conversion` is explicitly false or true. Repeat 1 tests false
then true within each load condition; repeat 2 reverses that order. Each
setting has a separate unpaced capacity run and a run paced at 120 fps, with
eight measurement seconds after 20 or 240 warmup frames respectively.
Entries below preserve **r1 / r2**. Latency is submission to observed encoder
output in milliseconds. The paced runs completed 120.105–120.113 fps.

| Compute | Load | Capacity, fps | Capacity mean, ms | Capacity p95, ms | Paced mean, ms | Paced p95, ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| false | Idle | 2149.409 / 2148.261 | 0.464 / 0.464 | 0.610 / 0.610 | 1.103 / 0.971 | 1.256 / 1.216 |
| true | Idle | 2227.784 / 2227.324 | 0.447 / 0.447 | 0.552 / 0.549 | 0.971 / 0.782 | 1.026 / 0.888 |
| false | GPU load | 1333.729 / 1326.874 | 0.748 / 0.752 | 0.875 / 0.912 | 0.959 / 0.953 | 1.292 / 1.087 |
| true | GPU load | 2107.694 / 2128.274 | 0.473 / 0.468 | 0.569 / 0.559 | 0.734 / 0.743 | 0.869 / 0.847 |

Every loaded case used the finite `gpu_load.exe 45 1000 60 200` fixture,
started two seconds before the probe. It remained alive through measurement
and exited naturally before the next case. All eight load summaries recorded
**2,701 frames at 60.0 fps** over the complete load run; this is the load
fixture's throughput, not GPU utilization or streaming throughput. Its frame
timings are retained separately from the encoder timings:

| Compute / probe | Load frame p50, r1 / r2 (ms) | Load frame p95, r1 / r2 (ms) |
| --- | ---: | ---: |
| false / capacity | 9.869 / 9.963 | 10.010 / 9.997 |
| false / paced | 9.895 / 9.778 | 10.096 / 9.982 |
| true / capacity | 9.971 / 10.193 | 10.631 / 10.940 |
| true / paced | 9.888 / 9.896 | 10.001 / 10.006 |

Raw evidence is retained in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\current-portable-load-20261012T051656834135Z`.
Each case retains the full-precision encoder JSON in `stdout.log`, its
`compute.conf`, `source.json`, exact arguments/status in `run.json`, and load
stdout/stderr where applicable. Stage metadata and the 16-entry summary
record completion. The task-local reproduction is
`work\run_current_matrix.py --stages portable-load`; its cases request
`--codec pyrowave --hdr --records --yuv444`, with `--paced` only for paced runs.

### Synthetic-motion AMF compute conversion under GPU load

All **16/16** cases passed from 07:47 to 07:50 CEST on October 12, using the
same published 2.2.9 performance executable and host above. This fixture cycles
through **16 generated BGRA8 pictures** with moving detail at the requested
output dimensions, converted to HDR10. It does not capture the physical
desktop or native HDR input, and excludes transport, independent decoding and
display latency. The JSON's generic `repeat-frame encoder throughput` label
does not distinguish this synthetic mode; `capture_backend=synthetic` and the
saved `--synthetic 16` arguments establish the actual source.

HEVC uses **1920×1080 at 60 fps / 20,000 kbps**; AV1 uses **1968×2184 at
120 fps / 80,000 kbps**. Both use AMF `ultralowlatency`, `speed` and
`vbr_latency`, paced for six measured seconds after 120 or 240 warmup frames.
The eight codec/load/compute combinations run once, then in reverse order for
repeat 2. AMF settings in every case confirm **D3D11/host input** with
`gpu_compute_conversion=false` and **D3D12 input** with it enabled. No
priority or direct-input variant is requested.

Entries preserve **r1 / r2**, in milliseconds. `encode_call` is the probe's
host-side encode-call duration, separate from submission to observed output;
neither is an isolated GPU hardware timer. Each HEVC run completed 361 frames
at 60.151–60.165 fps, and each AV1 run completed 721 frames at
120.134–120.147 fps. Measured durations were 6.000–6.002 seconds.

| Codec | Load | Compute | Encode-call mean | Submit-to-output mean | Submit-to-output p95 | Submit-to-output max |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| HEVC | Idle | false | 0.842 / 0.946 | 2.557 / 2.347 | 3.130 / 2.598 | 3.643 / 2.951 |
| HEVC | Idle | true | 1.081 / 1.103 | 2.078 / 2.034 | 2.571 / 2.154 | 2.622 / 2.666 |
| HEVC | GPU load | false | 0.960 / 0.870 | 7.570 / 8.728 | 12.647 / 15.587 | 14.111 / 21.365 |
| HEVC | GPU load | true | 1.089 / 1.099 | 2.047 / 2.104 | 2.201 / 2.332 | 2.888 / 5.605 |
| AV1 | Idle | false | 0.633 / 0.770 | 3.128 / 3.113 | 3.500 / 3.411 | 3.605 / 3.695 |
| AV1 | Idle | true | 1.094 / 1.112 | 2.932 / 2.868 | 3.125 / 3.121 | 3.592 / 3.828 |
| AV1 | GPU load | false | 0.903 / 0.962 | 7.621 / 7.840 | 14.612 / 14.915 | 22.941 / 21.994 |
| AV1 | GPU load | true | 1.194 / 1.134 | 2.928 / 2.914 | 3.137 / 3.138 | 4.644 / 6.871 |

The finite load is `gpu_load.exe 12 1000 0 1`: **12 seconds, 1,000 iterations,
uncapped, one draw**, started 1.5 seconds before the encoder process. These
are the archived October 4 runner's defaults. Its original invocation
overrides were not retained, and the original `target/release/examples`
load binary is absent; this run reuses the archived `bench-rc21` executable.
Consequently these are not matched old/new measurements. They also use a
different load from the capped 200-draw PyroWave fixture above.

All eight load processes remained alive through encoder completion and then
exited naturally with status zero. Their actual throughput and frame timings
over the complete load run follow; these are not encoder throughput or GPU
utilization. Entries again preserve **r1 / r2**.

| Codec / compute | Load frames | Load fps | Load frame p50, ms | Load frame p95, ms |
| --- | ---: | ---: | ---: | ---: |
| HEVC / false | 2353 / 2359 | 196.0 / 196.5 | 5.032 / 5.015 | 5.423 / 5.417 |
| HEVC / true | 2338 / 2368 | 194.8 / 197.3 | 5.080 / 5.023 | 5.407 / 5.336 |
| AV1 / false | 2326 / 2320 | 193.8 / 193.3 | 5.064 / 5.072 | 5.664 / 5.683 |
| AV1 / true | 2332 / 2331 | 194.3 / 194.2 | 5.106 / 5.107 | 5.441 / 5.436 |

Raw evidence is retained in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\synthetic-compute-20261012T054709781835Z`:
per-case `run.json`, `result.json`, `encoder.conf`, encoder stdout/stderr,
and load JSON/logs, plus stage metadata, hardware and the 16-entry summary.
The task-local reproduction is `work\run_synthetic_compute.py`; its SHA-256 is
`5d0245ff34b981de4a27966942c170f54c04e5bac2de6ca86d90d6631dcfc8c5`.
For example, the HEVC command is
`butterpollo-performance.exe --synthetic 16 --width 1920 --height 1080 --fps 60 --codec hevc --hdr --encoder amf --paced --seconds 6 --bitrate 20000 --config <case>\encoder.conf`.
The retained configurations and run records specify the compute toggle and
load command for each repeat.

### Isolated colour conversion and GPU correctness

These probes ran serially on the same host, using release-built test binaries
from the exact 2.2.9 source commit above. They exercise synthetic images and
individual functions, not the captured desktop or a complete HDR stream.

The CPU probe calls `color::hdr_rgba_scaled` on the historical synthetic
1968×2184 RGBA FP16 input, with one warmup call and three measurement seconds
per output. It excludes capture, encoding, transport and client work. This is
separate from the portable CPU-input encoder rows above.

| Output | Calls | Mean, ms | p95, ms | Maximum, ms |
| --- | ---: | ---: | ---: | ---: |
| 1920×1080 | 376 | 7.984 | 8.895 | 9.278 |
| 2560×1440 | 223 | 13.489 | 14.891 | 15.976 |
| 3840×2160 | 103 | 29.310 | 32.452 | 33.719 |

The independent `gpu_conversion_timing` fixture converts a synthetic
1968×2184 FP16 image to P010. It records 64 GPU timestamp pairs after 16
warmup conversions. GPU timestamps and CPU call times measure different
parts of the operation and are not additive.

| Output | GPU mean / p95, ms | CPU call mean / p95, ms |
| --- | ---: | ---: |
| 1968×2184 | 0.138 / 0.153 | 0.087 / 0.209 |
| 3840×2160 | 0.224 / 0.382 | 0.092 / 0.279 |

The D3D11 4:2:0 fallback fixture compares the retained pixel-pass reference
with the current single-pass converter. Each case uses an **ABBA** order,
128 GPU timestamp samples per run, and synthetic 3840×2160 BGRA8 or FP16
input. Each table entry is the arithmetic mean of the two run means for that
path, in milliseconds; it is not a percentile or a full-stream latency.
The loaded pass used the established `gpu_load.exe 60 1000 60 200` fixture,
which remained alive through the measurements. Video means NV12/P010 encoder
surfaces; planar means PyroWave planes.

| Output / format | Idle pixel passes | Idle single pass | Loaded pixel passes | Loaded single pass |
| --- | ---: | ---: | ---: | ---: |
| 1920×1080 SDR video | 0.246 | 0.198 | 0.340 | 0.294 |
| 1920×1080 SDR planar | 0.252 | 0.172 | 0.379 | 0.259 |
| 1920×1080 HDR video | 0.227 | 0.211 | 0.356 | 0.337 |
| 1920×1080 HDR planar | 0.243 | 0.191 | 0.364 | 0.284 |
| 2560×1440 SDR video | 0.312 | 0.155 | 0.470 | 0.239 |
| 2560×1440 SDR planar | 0.394 | 0.133 | 0.574 | 0.169 |
| 2560×1440 HDR video | 0.322 | 0.235 | 0.480 | 0.363 |
| 2560×1440 HDR planar | 0.388 | 0.223 | 0.577 | 0.322 |
| 3840×2160 SDR video | 0.255 | 0.129 | 0.352 | 0.190 |
| 3840×2160 SDR planar | 0.315 | 0.110 | 0.482 | 0.145 |
| 3840×2160 HDR video | 0.240 | 0.155 | 0.375 | 0.246 |
| 3840×2160 HDR planar | 0.309 | 0.133 | 0.460 | 0.205 |

All nine selected native correctness tests passed: compute/video parity,
compute/PyroWave-plane parity, single-pass/pixel-pass parity, disjoint video
plane UAVs, 4:4:4 chroma and ten-bit codes, SDR/HDR pointer composition,
absolute HDR luminance/gamut/linear resizing, retained-frame pool ownership,
and SDR ten-bit matrix/full-range handling. These native GPU tests validate
their synthetic fixtures; they do not establish native-HDR capture or
end-to-end HDR pixel correctness.

### FEC and video packet preparation

The packaged protocol probe reports the following current Rubylight FEC
medians over seven timing rounds. All four parity outputs matched the
existing reference byte for byte. No reference timing is refreshed here.

| Data / parity shards | Bytes per shard | Rubylight median, µs |
| --- | ---: | ---: |
| 32 / 7 | 1416 | 5.053 |
| 96 / 20 | 1416 | 43.271 |
| 192 / 39 | 1416 | 167.246 |
| 4 / 2 | 144 | 0.122 |

The packaged video-packet probe measures CPU packetization, 20% FEC,
optional AES-GCM and packet release, excluding capture, encoding and UDP
sending. Values are medians of seven 250 ms rounds. Fresh and reusable are
two buffer-allocation paths in the current build, not release comparisons.

| Data shards / payload bytes | Plaintext fresh, µs | Plaintext reusable, µs | AES-GCM fresh, µs | AES-GCM reusable, µs |
| --- | ---: | ---: | ---: | ---: |
| 32 / 44,024 | 8.147 | 5.621 | 15.439 | 12.821 |
| 192 / 264,184 | 158.085 | 145.109 | 199.857 | 189.920 |
| 576 / 792,568 | 476.564 | 433.632 | 605.496 | 565.106 |

### UDP pacing on Windows loopback

All **4/4** current-pacer component cases passed on October 12, from 07:41
to 07:42 CEST. The release-built `udp_pacing_probe` uses the same 2.2.9 source
commit, in `paced` mode, with an independent Python UDP receiver on
`127.0.0.1`. Each case sent exactly **600 synthetic frames, 46,042 data
packets and 64,458,800 UDP payload bytes**. Data packets are 1,400 bytes,
including the fixture header; the separate end marker is excluded from
those counts. The fixture requests 60 frames/s with a nominal 50 Mbps frame
budget and eight-times-sized keyframes at frames 0 and 300. The pacing
budgets are 100 or 800 Mbps, with a 0 or 3 ms scheduler stall after the second
packet batch of each keyframe.

Sender times cover each frame's batch-send loop, including pacing waits and
the imposed stall. Values are **mean / p99 / maximum, in milliseconds**.

| Pacing budget, Mbps | Stall, ms | Sender frame span, ms | Batch-send system calls |
| --- | ---: | ---: | ---: |
| 100 | 0 | 7.832 / 8.792 / 69.599 | 3062 |
| 100 | 3 | 7.837 / 8.798 / 72.264 | 3062 |
| 800 | 0 | 0.452 / 0.634 / 8.115 | 1222 |
| 800 | 3 | 0.449 / 0.508 / 10.842 | 1222 |

The receiver timestamps userspace `recvfrom` completion with
`time.perf_counter_ns`. Frame span runs from the first to last received
packet; completion gap runs between completed frames in frame-ID order.
Both distributions include the two large keyframes. The receive buffer was
4,194,304 bytes in all cases. Values again show **mean / p99 / maximum**.

| Pacing budget, Mbps | Stall, ms | Receiver frame span, ms | Completion gap, ms | Gaps over 25 / 50 ms |
| --- | ---: | ---: | ---: | ---: |
| 100 | 0 | 7.773 / 8.005 / 69.320 | 16.563 / 16.725 / 78.628 | 1 / 1 |
| 100 | 3 | 7.804 / 8.025 / 72.128 | 16.558 / 16.712 / 81.396 | 1 / 1 |
| 800 | 0 | 0.498 / 0.669 / 7.971 | 16.653 / 16.837 / 24.290 | 0 / 0 |
| 800 | 3 | 0.501 / 0.537 / 11.009 | 16.648 / 16.774 / 27.027 | 1 / 0 |

Every case received all 46,042 unique packets and all 600 frames intact,
with zero missing, duplicate, corrupt or malformed data packets, partial
frames or sender-reported drops. Passing here means exact delivery and
integrity, not an absence of timing gaps. `kernel_overflow` is **unavailable
(`null`)**, not a measured zero. Python scheduling and receive processing
affect these userspace spans and gaps; they are not kernel arrival timestamps
or one-way latency. This Windows loopback component excludes capture,
codecs, FEC, the physical NIC, LAN/Wi-Fi and client display. It does not
replace real-client or historical pacing measurements.

Raw evidence is retained in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\udp-pacing-loopback-20261012T054151299614Z`.
Each case contains `sender.json`, `receiver.json` with every frame's timing,
`run.json` and process logs. `manifest.json` records exact commands and hashes
for the source-built sender and adapted Windows receiver. The sender SHA-256
is `bc7f7decfd1718c0fec92c5d3e2355ab470ef383d5ba4856ae72436b911ac565`.
The retained task-local runner is `work\run_udp_pacing.py`, using
`udp_pacing_probe.exe <loopback-peer> paced <100-or-800> <0-or-3>`.

### Wait and input components

`wait_performance` recorded 40 samples per method and requested duration.
Entries are observed wait **mean / p95 in milliseconds**, not additional
stream latency or overshoot alone.

| Requested wait, µs | Condition variable | Thread sleep | Capture event + timer | Input timer |
| --- | ---: | ---: | ---: | ---: |
| 250 | 15.413 / 16.172 | 0.517 / 0.541 | 0.512 / 0.525 | 0.514 / 0.544 |
| 500 | 15.492 / 16.103 | 1.050 / 1.213 | 1.038 / 1.407 | 0.540 / 0.969 |
| 1000 | 15.232 / 16.299 | 1.524 / 1.571 | 1.519 / 1.558 | 1.082 / 1.517 |
| 16667 | 30.775 / 31.955 | 16.965 / 17.229 | 16.957 / 17.178 | 16.930 / 17.139 |

Input probes used HIGH process priority, 1 ms timer resolution and disabled
power throttling. They inject zero relative mouse movement or neutral
controller states; these are host component measurements, not input-to-display
latency. Control decrypt-and-decode averaged 0.3 µs for both protocol variants
over 20,000 datagrams each, with p99 of 0.4 µs for `v2=true` and 0.5 µs for
`v2=false`.
Idle `SendInput` batching produced the following per-input times, with zero
failed calls:

| Inputs per call | Calls | Mean per input, µs | p99 per input, µs | Maximum per input, µs |
| --- | ---: | ---: | ---: | ---: |
| 1 | 1500 | 30.6 | 89.8 | 156.8 |
| 2 | 750 | 16.0 | 56.5 | 74.6 |
| 4 | 375 | 8.3 | 43.9 | 51.9 |
| 8 | 200 | 6.2 | 37.5 | 41.3 |

Under TIME_CRITICAL spinners on every CPU, the receiver-priority fixture
recorded 1,500 loopback input samples for each current scheduling variant:

| Receiver variant | Mean, µs | p99, µs | Maximum, µs |
| --- | ---: | ---: | ---: |
| Poll1 PriorityNew | 7501.6 | 24855.7 | 32111.7 |
| Poll1 PriorityInput | 17.4 | 30.0 | 211.0 |
| Poll1 MmcssOnly | 25.9 | 33.7 | 214.0 |

The separate queued-controller fixture measures completion of a zero mouse
move immediately after a neutral controller state through the injector.
Its normal/highest worker loads are distinct from the TIME_CRITICAL fixture.

| Worker load | Samples | Mean, µs | p99, µs | Maximum, µs |
| --- | ---: | ---: | ---: | ---: |
| None | 6000 | 49.6 | 204.8 | 652.9 |
| Normal | 5949 | 50.1 | 130.3 | 2128.3 |
| Highest | 5887 | 57.8 | 167.4 | 2261.3 |

Direct synchronous VHF state calls remain a different scope, with two rounds
of 1,000 calls each per worker load:

| Worker load / round | Mean, µs | p99, µs | Maximum, µs |
| --- | ---: | ---: | ---: |
| None / 1 | 103.1 | 145.9 | 211.0 |
| None / 2 | 100.0 | 129.4 | 217.6 |
| Normal / 1 | 81.7 | 170.4 | 4680.8 |
| Normal / 2 | 100.6 | 2035.8 | 10442.7 |
| Highest / 1 | 559.5 | 2072.6 | 218535.4 |
| Highest / 2 | 306.5 | 2069.7 | 212881.8 |

Direct driver calls still showed a **218.535 ms** tail, while the
queued controller/mouse fixture above had a **2.261 ms** maximum. Passing the
probe does not establish a bound on driver-call latency. VHF open/first
neutral state/drop took 3281.6 / 954.9 / 407.4 µs; the neutral pad was removed
at completion.

Two ENet loopback repeats held acknowledgments until after apply and used
3,000 samples each. Both recorded zero socket sends inside the service drain.

| Repeat | Client send+flush → apply, mean / p99 µs | Server wake → drain end, mean / p99 µs |
| --- | ---: | ---: |
| 1 | 34.7 / 51.4 | 8.8 / 17.0 |
| 2 | 34.8 / 51.5 | 9.1 / 17.0 |

Two additional **immediate-ACK** reference runs completed on October 12 at
09:37 UTC, with the same exact-source executable and original return-code /
30-second timeout gates. Each sent 3,000 reliable 36-byte loopback packets;
the server uses HIGH priority and a 1 ms timer. These ran later than the
deferred-ACK pair above and are **not an alternating matched comparison**.
No input-latency threshold was added to the gate.

| Immediate-ACK repeat | Send+flush → apply, mean / p99 / max µs | Wake → drain end, mean / p99 / max µs | Socket send time in drain, mean / p99 µs |
| --- | ---: | ---: | ---: |
| 1 | 48.9 / 72.5 / 99.7 | 22.9 / 36.4 / 57.9 | 16.2 / 24.2 |
| 2 | 49.7 / 73.2 / 243.8 | 23.3 / 36.9 / 201.0 | 16.5 / 24.7 |

Both immediate runs averaged **1.0 socket send** inside the drain, with a
maximum of two. Those are counts, despite the probe's generic `us` suffix
on count-labelled lines. Raw evidence is in
`work\measurements\enet-immediate-20261012T093736Z`; the executable SHA-256 is
`c9195204c667cec0b2e0c0a0fb40be2f2824c2ba55b71f03fb8ba1989571ec3f`.
The two original deferred runs and their measurements remain unchanged.

### PyroWave decoded pictures and quality budgets

Five serial source-built tests, using the published package's
`libpyrowave-shared-1.dll` and bitstream `186f0393`, completed all **7,200
expected decoded comparisons**. The log audit found no missing, duplicate or
unexpected sample combinations. These tests encode synthetic pictures,
packetize them, reconstruct complete delivery in an independent parser, then
decode through Vulkan and read back the planes. They exclude live network
loss, socket pacing, sustained frame rate and client display latency.

| Test | Decoded comparisons / expected | Result | Test duration, s |
| --- | ---: | --- | ---: |
| Chart, 720p and 1080p | 1,152 / 1,152 | Pass | 73.48 |
| Quality, 720p, default critical FEC | 1,872 / 1,872 | Pass | 126.58 |
| Quality, 1080p, default critical FEC | 1,872 / 1,872 | Pass | 158.11 |
| Quality, 2160p, default critical FEC | 1,872 / 1,872 | Pass | 323.60 |
| Quality, 2160p, critical FEC disabled | 432 / 432 | Pass | 100.95 |

The chart covers SDR8, SDR10 and HDR10, both chroma formats, and requested
30/125/400/1000 Mbps budgets. Its passing assertions require identical decoded
samples across graphics/compute conversion, plain/record framing and three
alternating batches. The separate quality sweeps use compute conversion,
record framing, 1392-byte packets, 20% critical FEC and no detail FEC, except
for the explicitly labelled no-FEC control. Each encoder interval is reset
before the picture, so nominal 30/60/120 fps defines its byte budget rather
than measuring sustained throughput. Quality scores and frame sizes matched
exactly across both repeats and all three nominal frame rates.

Current warning/recommended budgets, in bits per pixel per frame (bpp),
remain **2.5/5 bpp below 1080 lines** and
**1.5/3.2 bpp at 1080 lines and above**. Every row below met the warning-floor
criterion and the recommended-quality criterion in **24/24 desktop/game
comparisons at each level**: two scenes, two pictures, two repeats and three
frame rates. The floor requires luma PSNR ≥25 dB and luma SSIM ≥0.80; the
recommendation requires every plane's PSNR ≥35 dB and luma SSIM ≥0.95.

| Size | Output | Floor / recommended bpp | Recommended minimum plane PSNR / luma SSIM | Derived RTP Mbps at 60 fps |
| --- | --- | ---: | ---: | ---: |
| 720p | SDR8 4:2:0 | 2.5 / 5 | 41.82 / 0.9751 | 236.5–295.3 |
| 720p | SDR8 4:4:4 | 2.5 / 5 | 39.99 / 0.9672 | 227.8–286.6 |
| 720p | HDR10 4:2:0 | 2.5 / 5 | 48.56 / 0.9946 | 289.3–331.8 |
| 720p | HDR10 4:4:4 | 2.5 / 5 | 46.83 / 0.9907 | 285.9–303.5 |
| 1080p | SDR8 4:2:0 | 1.5 / 3.2 | 38.92 / 0.9721 | 412.3–424.4 |
| 1080p | SDR8 4:4:4 | 1.5 / 3.2 | 38.26 / 0.9653 | 412.9–441.3 |
| 1080p | HDR10 4:2:0 | 1.5 / 3.2 | 46.17 / 0.9912 | 412.3–458.9 |
| 1080p | HDR10 4:4:4 | 1.5 / 3.2 | 45.23 / 0.9894 | 412.9–458.9 |
| 2160p | SDR8 4:2:0 | 1.5 / 3.2 | 44.73 / 0.9910 | 1645.0–1737.6 |
| 2160p | SDR8 4:4:4 | 1.5 / 3.2 | 40.68 / 0.9747 | 1649.7–1710.6 |
| 2160p | HDR10 4:2:0 | 1.5 / 3.2 | 50.88 / 0.9956 | 1649.7–1884.9 |
| 2160p | HDR10 4:4:4 | 1.5 / 3.2 | 49.24 / 0.9936 | 1643.6–1884.9 |

PSNR and SSIM minima can come from different pictures. PSNR uses peaks of
255/1023; HDR is measured in PQ code space. SSIM uses uniform 8×8 windows.
These synthetic-scene targets do not establish visually perfect output.
The RTP ranges cover 12 samples per row at the recommended budget, including
the dark scene: three scenes, two pictures and two repeats at nominal 60 fps.
They are measured datagram bytes converted to bits and scaled by 60 fps,
including FEC but excluding
UDP/IP/link headers; they are **not measured network throughput**. Ranges are
observed minima/maxima, not confidence intervals.

At 4K, the default-FEC encoder budget remained capped at **3,921,592 bytes**
for all requested 4/4.5/5 bpp points. The separate no-FEC control allowed
4,147,196 / 4,665,596 / 5,183,996 bytes respectively, with 144/144 decoded
comparisons at each point for each FEC setting. These requested high budgets
do not establish that a live client can negotiate or sustain those rates.

Raw evidence is under
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\remaining-probes-20261012T043354Z`,
in `pyrowave-decoded`, `pyrowave-quality-720p`, `pyrowave-quality-1080p`,
`pyrowave-quality-2160p` and `pyrowave-quality-2160p_without_fec`. Each has
`stdout.log`, `stderr.log` and `run.json`; the stage manifest pins the test
binary and published DLL hashes. The task-local
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\pyrowave-summary.json`
retains the coverage audit, unrounded scores, byte ranges and repeat checks,
produced by `work\summarize_pyrowave.py` without changing the evidence.

### Retained AMF property sweeps (October 12)

The published 2.2.9 performance executable completed **1,030 serial cases**
between **10:21 and 13:04 UTC**: **30/30** low-latency-property cases,
**280/280** rate-control/IDR cases and **708/720** split-frame cases passed.
These ran on the Ryzen 7 5800X3D / RX 7900 XT, AMD driver
`32.0.32015.2008`, Windows build 26200. All 12 failures remain recorded;
there were no interrupted attempts or measurement retries.

Each process cycles 16 deterministic synthetic BGRA8 pictures through GPU
conversion and AMF. HDR rows convert those SDR pictures to HDR output.
This excludes capture, network, independent decoding and display latency;
it does not measure perceptual quality. Paced cases warm up for two seconds;
unpaced cases warm up for 20 frames. The original gate checks process return
code, requested fixture identity, measured duration, and equality of submitted
frames, completed frames and latency samples. **Pass does not mean that a
requested cadence or latency threshold was met**, or that every optional
property took effect. Installed-stream and competing-workload guards ran
around the cases; the installed host was also checked during each case.

#### Low-latency properties

All runs use **2560x1440 SDR / 120 FPS / 20 Mbps**, eight measurement seconds.
`auto` is paired with `amd_lowlatency_mode=enabled` for H.264/HEVC or
`amd_av1_latency_mode=lowest` for AV1. The ultralowlatency usage has three
repeats per arm; the other usages have one. Values below are **auto / forced**,
with latency in milliseconds. For the three-repeat rows, each number is the
arithmetic mean of the per-run statistic, not a pooled percentile.

| Usage | Codec | Runs per arm | Submission-to-output mean, auto / forced | p99, auto / forced | Driver property readback, auto / forced |
| --- | --- | ---: | ---: | ---: | ---: |
| ultralowlatency | H.264 | 3 | 3.107 / 3.064 | 3.608 / 3.585 | 1 / 1 |
| ultralowlatency | HEVC | 3 | 3.229 / 3.213 | 3.658 / 3.624 | 1 / 1 |
| ultralowlatency | AV1 | 3 | 2.634 / 2.659 | 3.105 / 3.115 | 3 / 3 |
| lowlatency | HEVC | 1 | 3.539 / 3.228 | 3.941 / 3.636 | 0 / 1 |
| lowlatency | AV1 | 1 | 3.649 / 2.641 | 4.130 / 3.082 | 0 / 3 |
| lowlatency_high_quality | HEVC | 1 | 3.567 / 3.567 | 3.966 / 3.986 | 1 / 1 |
| lowlatency_high_quality | AV1 | 1 | 3.476 / 3.493 | 3.909 / 3.906 | 3 / 3 |
| transcoding | HEVC | 1 | 3.543 / 3.240 | 3.957 / 3.687 | 0 / 1 |
| transcoding | AV1 | 1 | 4.005 / 2.640 | 4.437 / 3.098 | 0 / 3 |

The readback is `LowLatencyInternal` for H.264/HEVC and
`Av1EncodingLatencyMode` for AV1. Current ultralowlatency defaults already
read enabled/lowest; measured times are similar, not numerically identical.
The single lowlatency/transcoding pairs show lower means when forcing the
property, but do not establish a quality tradeoff, confidence interval or
reason to change the default usage. The old table did not retain a duration;
this rerun uses the current probe's eight-second default and does not claim
an exact historical timing comparison.

#### Rate control and recovery-frame sizes

The 280 runs preserve **140 unique codec/fixture/configuration conditions,
two repeats each**. Main rows use HEVC/AV1 SDR at 1080p60, 1440p120 and 4K60,
20/40/80 Mbps, with default, CBR, CBR+VBV1, cap1, cap2 and intra-refresh
settings. Frequent-IDR rows use 4K60 / 63 Mbps and an IDR every six frames.
The peak/VBV recheck uses 4K60 / 40 Mbps. These request eight measurement
seconds after two seconds of warmup. Duplicate default rows are represented
once; historical contamination replacements and private development
screening are not additional current conditions.

In the following table, ranges are **minimum–maximum per-run observations
over each setting's actual retained conditions**, not pooled distributions
or matched comparisons between differently populated rows. Latencies are
milliseconds. Actual/requested bitrate is encoded bytes × 8 / measured
seconds / requested bitrate. IDR ratio is the largest measured IDR divided
by the same run's mean non-IDR frame size; startup IDRs are separate.

| Setting | Runs | Mean submission-to-output range | p99 range | Actual/requested bitrate range | Largest-IDR / mean non-IDR range |
| --- | ---: | ---: | ---: | ---: | ---: |
| default | 40 | 1.779–6.196 | 1.865–6.703 | 0.936–1.018 | 1.45–3.43 |
| cbr | 36 | 1.777–6.209 | 1.871–6.719 | 0.986–1.011 | 1.20–3.03 |
| cbr_vbv1 | 40 | 1.780–6.155 | 1.870–6.696 | 0.973–1.072 | 1.18–3.17 |
| cap1 | 40 | 1.755–6.176 | 1.813–6.683 | 0.810–1.001 | 0.85–1.35 |
| cap2 | 40 | 1.757–6.193 | 1.802–6.729 | 0.936–1.007 | 1.45–2.35 |
| intra | 36 | 1.756–6.187 | 1.822–6.709 | 0.932–1.006 | no measured IDR |
| cbr_cap1 | 4 | 4.967–6.009 | 5.392–6.637 | 0.869–0.921 | 1.36–1.74 |
| cbr_cap2 | 4 | 5.007–6.059 | 5.422–6.675 | 1.028–1.042 | 1.65–2.39 |
| peak1 | 4 | 4.906–6.000 | 5.348–6.422 | 0.966–0.985 | 1.94–3.33 |
| peak15 | 4 | 4.925–5.993 | 5.346–6.404 | 0.966–0.985 | 1.94–3.33 |
| peak2 | 4 | 4.927–5.979 | 5.329–6.365 | 0.966–0.985 | 1.94–3.33 |
| vbv05 | 4 | 4.943–5.964 | 5.282–6.613 | 0.958–0.965 | 2.08–3.78 |
| vbv1 | 4 | 4.943–6.007 | 5.372–6.504 | 0.966–0.985 | 1.94–3.33 |
| vbv2 | 4 | 4.956–5.992 | 5.370–6.413 | 0.969–0.999 | 1.94–3.01 |
| cap4 | 4 | 4.919–5.995 | 5.346–6.856 | 0.966–0.985 | 1.94–3.33 |
| hrd | 4 | 4.895–6.000 | 5.344–6.596 | 0.966–0.985 | 1.94–3.33 |
| cap2hrd | 4 | 4.948–5.993 | 5.316–6.623 | 0.964–0.969 | 1.93–2.10 |
| intra_idr | 4 | 4.931–5.957 | 5.308–6.374 | 0.963–0.986 | 1.94–3.34 |

Names map to the saved configurations: `cbr` sets `amd_rc=cbr`; `vbv05`,
`vbv1`, `vbv2` set `amd_vbv_buffer_frames` to 0.5, 1, 2; `cap1`, `cap2`,
`cap4` set `amd_max_frame_size` to 1, 2, 4; `peak1`, `peak15`, `peak2`
set `amd_peak_bitrate_ratio` to 1, 1.5, 2. `hrd` sets `amd_enforce_hrd=true`.
`cbr_cap1/2` combine CBR, VBV1 and that cap; `cap2hrd` combines cap2 and HRD.
`intra` uses `--intra-refresh` without periodic IDRs; `intra_idr` keeps the
one-second IDR interval. The saved per-condition index preserves the exact
codec, dimensions, rate, bitrate, IDR interval and requested/applied fields.

The logs retain actual driver readbacks rather than inferring them from
the configuration. For example, 4K60 / 40 Mbps peak1/1.5/2 reads peak bitrates
of 40/60/80 Mbps; cap1/2 reads 666,667 / 1,333,333 **bits**, not bytes.
Observed bitrate and IDR ratios remain measurements, not hard bounds or
proof of visual quality. No recovery decoder or packet-loss test runs in
this component fixture.

#### Split-frame settings

All **240 fixture/repeat slots** ran current release `auto`, `enabled` and
`disabled`, for 720 cases. Main: 60 fixtures × two repeats × three modes;
interaction: 37 × two × three; unpaced throughput: eight × two × three;
stability: six × five × three. Main/interaction request three measurement
seconds, throughput four, stability eight. The retained fixtures include
HEVC/AV1, SDR and converted-HDR output, five widths from 1920 to 7680,
60/120/240 FPS requests, and 20/40/80/150 Mbps targets, plus their saved
slices, intra-refresh, LTR, quality, preanalysis, usage, Smart Access Video
and rate-control interactions. Current `auto` replaces the old private
untouched-binary arm; it is not that historical binary.

The table gives min–max per-run latency observations over each subgroup's
completed cases, not averages across different fixtures. Failures remain
in the denominators and have no accepted latency values.

| Subgroup | Mode | Passed / attempted | Mean submission-to-output range, ms | p99 range, ms |
| --- | --- | ---: | ---: | ---: |
| main | auto | 120 / 120 | 1.702–87.228 | 1.768–89.815 |
| main | enabled | 120 / 120 | 1.702–87.131 | 1.752–89.794 |
| main | disabled | 120 / 120 | 1.711–87.133 | 1.761–89.799 |
| interaction | auto | 70 / 74 | 4.833–43.527 | 5.184–72.844 |
| interaction | enabled | 70 / 74 | 4.835–43.575 | 5.194–72.919 |
| interaction | disabled | 70 / 74 | 4.804–43.585 | 5.192–73.229 |
| throughput | auto | 16 / 16 | 9.804–87.901 | 10.513–89.735 |
| throughput | enabled | 16 / 16 | 9.800–87.837 | 10.485–89.724 |
| throughput | disabled | 16 / 16 | 9.795–87.937 | 10.498–89.725 |
| stability | auto | 30 / 30 | 3.117–6.145 | 3.592–6.692 |
| stability | enabled | 30 / 30 | 3.098–6.166 | 3.601–6.688 |
| stability | disabled | 30 / 30 | 3.089–6.149 | 3.600–6.695 |

Unpaced capacity below is the arithmetic mean of two runs for each identical
fixture. All eight throughput fixtures are SDR; the FPS values are encoded
capacity, not independently decoded or fresh captured pictures.

| Codec / SDR output / Mbps | Auto FPS | Enabled FPS | Disabled FPS |
| --- | ---: | ---: | ---: |
| HEVC / 1920x1080 / 20 | 669.11 | 669.44 | 668.89 |
| HEVC / 3840x2160 / 80 | 197.32 | 197.37 | 197.36 |
| HEVC / 5120x1440 / 80 | 217.93 | 217.92 | 217.97 |
| HEVC / 7680x2160 / 150 | 102.05 | 102.07 | 102.06 |
| AV1 / 1920x1080 / 20 | 842.40 | 842.81 | 842.81 |
| AV1 / 3840x2160 / 80 | 250.45 | 250.51 | 250.48 |
| AV1 / 5120x1440 / 80 | 273.81 | 273.73 | 273.76 |
| AV1 / 7680x2160 / 150 | 127.04 | 126.88 | 126.96 |

All 720 split-frame readbacks were present: `auto` and `enabled` read
`MultiHwInstanceEncode=1`, while `disabled` read 0, with the codec-specific
prefix. A matching property value does not prove how much work each hardware
engine performed. Throughput was similar across these arms, not identical.
**66 passed paced cases completed below 97% of requested FPS**; this is a
descriptive observation, not a retroactively added gate. For example, HEVC
7680x2160 SDR at a requested 240 FPS completed **102.532–102.733 FPS** across
its six main-matrix runs. The high submission-to-output tails above include
such capacity-limited fixtures and are not display latency.

All 12 failures are the retained `amd_preanalysis=true` interaction at
**3840x2160 SDR / 60 FPS / 80 Mbps**, for both codecs, all three split-frame
settings and both repeats:

| Codec / interaction row | Nonzero probe exits | Original sample-accounting failures | Accepted timing results |
| --- | ---: | ---: | ---: |
| HEVC / 7 | 6 | 0 | 0 |
| AV1 / 19 | 4 | 2 | 0 |

The ten nonzero exits report `encoder did not complete measured frames`.
AV1 `auto` run 1 and `disabled` run 1 return zero with 181 submitted and
completed frames but **180 latency samples**, failing the unchanged equality
gate. Their raw JSON is retained as failed-case diagnostics and excluded
from accepted timing aggregates. The results do not isolate a cause to
split-frame mode: every mode failed this preanalysis condition. No run was
retried, fixed or silently dropped.

Across the full property sweep, the logs retain **78 AV1 1080p geometry
warnings** and **six HEVC LTR-setting warnings**. The former warn about AMD
padding and client cropping; the latter state that `amd_ltr_frames` applies
only to AV1 and the HEVC request is ignored. There were no logged
`AMF setting unavailable` warnings. Completion and primary-property
readback therefore remain distinct from support for every requested
interaction, independent decoding or visual quality.

#### Property evidence and validation

Raw results are in
`work\measurements\amf-properties-20261012T102110Z`, produced by
`work\run_amf_properties.py` (SHA-256
`b11eabc9a2ee9d76dbb858a3582353b31fa60e1ccb6dc8359603aa8922fddf66`).
The executable is the published package probe identified above. Each case
retains its exact configuration, arguments, stdout/stderr, status and hashes;
`planned.json` and `manifest.json` index all 1,030 cases. The saved CSVs and
dated rate-control tables are pinned in the manifest, with current source
and package identities.

`work\amf-properties-summary.json`, generated by
`work\summarize_amf_properties.py`, verifies all **1,030 run records and
4,110 raw-file hashes** against the original evidence and rechecks the
original gates. It indexes every case, requested/applied field, warning and
failure, plus **491 distinct fixture/configuration aggregates** with
per-run ranges. Only identical repeats are averaged; no percentiles are
pooled, and no failed result is promoted to a pass. The same summary
retains the two immediate-ACK ENet runs and their original gates/provenance.
Historical and competitor measurements remain unchanged.

### Reproduction and retained evidence

From the extracted 2.2.9 package, the following commands reproduce the
1440p HEVC HDR-output triple with the recorded requested settings:

```powershell
.\butterpollo-performance.exe --encoder amf --capture wgc --codec hevc --width 2560 --height 1440 --fps 120 --bitrate 50000 --seconds 8 --hdr
.\butterpollo-performance.exe --encoder amf --capture wgc --codec hevc --width 2560 --height 1440 --fps 120 --bitrate 50000 --seconds 8 --hdr --paced
.\butterpollo-performance.exe --encoder amf --capture wgc --codec hevc --width 2560 --height 1440 --fps 120 --bitrate 50000 --seconds 8 --hdr --cpu
```

Use the dimensions and bitrates above for the other rows, `--codec h264`,
`--codec hevc`, `--codec av1` or `--codec pyrowave`, and omit `--hdr` for SDR.
PyroWave additionally uses `--records`, plus `--yuv444` for its 4:4:4 rows.
Keep the source, display state and lack of competing workloads consistent;
the recorded source was SDR even when `--hdr` was requested.

Local raw evidence is retained in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\current-portable-20261012T041533202030Z`.
Each `<width>x<height>-<codec>-gpu-capacity`, `-gpu-paced` or `-cpu-capacity`
directory contains `stdout.log` with the full measurement JSON, `stderr.log`,
`run.json` with arguments, status and timestamps, and `source.json` with the
actual display state. Stage `metadata.json`, `hardware.json` and `summary.json`
retain executable hashes, the driver/OS inventory and all 81 completion records.
The task-local runner is `work\run_current_matrix.py --stages portable`.

The component results above are retained separately in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\remaining-probes-20261012T043354Z`.
Its `manifest.json` records source revision, package/test-binary hashes and
completion status; each named case has `run.json` with the exact executable,
arguments and timestamps, plus stdout/stderr. The FEC values above come only
from `fec\rubylight-only.json`; GPU timing comes from each case's `gpu.json`.
The CPU fixture source and its input specification are retained under
`work\cpu-color-probe`. Input diagnostic detail not tabulated above remains
in the complete logs.

Reproduce the source-built components with the existing `wait_performance`,
`input_path_probe` and `enet_input_probe` release examples from the recorded
commit. Input modes were the default invocation, `vhf --load=none,normal,highest --trace`,
`pad-contention --load=none,normal,highest --trace`, and
`contention --samples=1500`; ENet used `3000 defer` twice, followed later by
two separate `3000` immediate-ACK reference runs. The isolated CPU
fixture had no arguments. Native GPU cases used their `gpu_color::tests`
names with `--exact --ignored --nocapture --test-threads=1` and wrote timing
reports through `BUTTERPOLLO_TEST_GPU_REPORT`. Run these serially and keep
the explicitly loaded GPU pass separate from idle measurements. These
source-built tests supplement the published-package probes; they are not
additional downloaded release executables.

## October 14 HDR and recovery fixes, 2.2.18

Compared the published 2.2.17 portable executable with the 2.2.18 source-built
release example on 2026-10-14: Ryzen 7 5800X3D, Radeon RX 7900 XT, AMD driver
32.0.32015.2008, Windows 11 build 26200 and pinned Rust 1.98.1 GNU. These were
component probes on an idle host, not streams through the installed service.
The baseline ZIP SHA-256 was
`80e3b7b6feec4bd2a252d76e203ff220a4ca776c60f4b0d95698f19e0cc63302`.

Each row has three six-second measurements per build after a two-second
warmup. Values below are the median of each run's mean, p95 and p99; they are
not percentiles pooled across runs. Native AMF used 1920×1080, 120 FPS and
20 Mbps with 16 synthetic pictures, SDR for H.264 and HDR output for HEVC/AV1.
WGC used AV1 HDR output at the same settings, with both builds explicitly
capturing the same 1092×984 desktop. This static-source check does not establish
fresh-picture delivery or native HDR capture performance.

| Native AMF workload | 2.2.17 mean / p95 / p99 (ms) | 2.2.18 mean / p95 / p99 (ms) | Encoded FPS before / after |
| --- | ---: | ---: | ---: |
| Synthetic H.264 SDR | 2.213 / 2.587 / 2.645 | 2.208 / 2.625 / 2.678 | 120.15 / 120.14 |
| Synthetic HEVC HDR | 2.176 / 2.597 / 2.644 | 2.201 / 2.635 / 2.792 | 120.14 / 120.14 |
| Synthetic AV1 HDR | 1.801 / 1.909 / 5.089 | 1.835 / 2.105 / 2.586 | 120.13 / 120.14 |
| WGC to AV1 HDR | 1.870 / 2.289 / 2.684 | 1.889 / 2.593 / 2.662 | 120.13 / 120.14 |

Native timings run from the encode request, including capacity checks and
colour conversion, to observed output. They exclude capture age, network,
decoding and display latency. The requested cadence was
maintained; short-run tail timings were mixed, including higher HEVC p99 and
WGC p95. These results do not establish a latency improvement or a universal
non-regression result. Earlier WGC runs used different active display sizes
(5120×1440 versus 1092×984); they were retained separately and excluded from
this comparison. The replacement WGC runs alternated the two builds and
checked source dimensions after every run.

Software HEVC used 1280×720 SDR, 60 FPS, 10 Mbps, CPU input and a recovery
request every 60 measured submissions. Its reported metric is the duration
of the **encode call**, not the native request-to-output metric above:

| Software workload | 2.2.17 mean / p95 / p99 (ms) | 2.2.18 mean / p95 / p99 (ms) | Encoded FPS before / after |
| --- | ---: | ---: | ---: |
| x265 with periodic recovery | 2.869 / 2.897 / 12.685 | 2.960 / 2.945 / 13.159 | 60.14 / 60.14 |

The separate real x265 regression exposed missing recovery keyframes when
requests arrived only a few frames apart. With explicit IDR output enabled,
the unchanged expected sequence `[true, false, true, false, false, true, false]`
passes, including metadata changes, unchanged/control-only metadata and an
explicit recovery request. Its eight-bit fixture verifies encoded keyframe
flags, not HDR pixels or NVENC behavior. The six-second performance runs have
more widely spaced requests and are not the proof of that regression fix.

Native AMD validation independently parsed HEVC/AV1 mastering and content-light
metadata, BT.2020/PQ and full/limited range with FFprobe over 40 encoder
configurations. Metadata updates alone produced the expected keyframes at
frames 4 and 8, with no extra keyframe on unchanged frames. The earlier native
HDR test also passed on the baseline. Seven additional native checks covered
headless hardware encoding, WGC update intervals and reconnects, DDX snapshot
immutability, full snapshot pools, D3D11 fallback and compute texture ownership.
Ordinary tests exercise driver rejection/retry, per-property bitrate scaling,
snapshot backpressure and NVENC API/metadata bookkeeping. NVIDIA hardware,
live SDR/HDR switching, client presentation and physical TV calibration were
not tested for this release.

Reproduce the native synthetic probe with
`butterpollo-performance.exe --encoder amf --codec av1 --hdr --width 1920 --height 1080 --fps 120 --bitrate 20000 --seconds 6 --synthetic 16 --paced`.
Use `--codec h264` without `--hdr` or `--codec hevc` for the other codecs.
For WGC replace `--synthetic 16` with `--live-capture --capture wgc --display NAME`
and pin the same active output on both builds. For software use
`--encoder software --codec hevc --width 1280 --height 720 --fps 60 --bitrate 10000 --seconds 6 --synthetic 16 --cpu --paced --idr-interval 60`.

Raw JSON, stderr, executable hashes, hardware inventory and test logs are
retained under
`C:\Users\ramaz\Documents\Codex\2026-10-13\on-ruby-lite-or-ruby-lite\outputs\release-2.2.18`.
`performance-summary.json` records the aggregates; `unmatched-display-*`
contains the excluded WGC runs. `bench.ps1`, `bench-software.ps1`, `compare.py`
and `native-tests.ps1` record the component commands.

## October 14 conventional video pacing

The fast-start results below record the earlier candidate, superseded before
2.2.19 was published. The [conservative startup validation](#conservative-startup-before-2219)
separates the final opening-frame cost from steady-state timing.

The first candidate applied roughly 2× encoder-bitrate pacing to every
conventional-codec route. It was **rejected before release**: a valid Windows
loopback comparison measured mean frame-send time of 0.527 ms at the released
800 Mbps rate versus 10.439 ms at the candidate's 80 Mbps rate. A modeled
slow egress benefited, but that did not justify the delay on healthy paths.

The replacement preserves the fast starting rate on wired/unknown routes
and reduces bursts only after repeated validated client FEC loss reports.
It never writes encoder bitrate. It reserves the ordered sender at startup
so later pacing waits do not block capture. A live wire-demand floor,
distinct-frame accounting, stale/pre-change feedback fences and cautious
recovery keep FEC changes or delayed reports from creating an artificial
quality cap. Explicit pacing, PyroWave and opt-in host bitrate adaptation
keep their separate policies; see [configuration](../docs/configuration.md#capture-and-video).

### Isolated Windows UDP comparison

Three alternating runs per case used the released 2.2.18 source
`0721060028dfff6cb710a9264e3938e0f9d85d56` and the replacement candidate.
Both probes omit `StreamingScope::enter`, which changes desktop/Wi-Fi/GPU
policy, and run at ordinary process priority. No capture, installed service,
display changes or game were used. The fixture is 50 Mbps of 1400-byte UDP
payload at 60 FPS, 600 frames including two 8× keyframes. It approximates
40 Mbps encoded video plus FEC/framing; it does not encode pictures or
generate actual parity. The automatic case exercises the production rate
and controller under the per-frame mutex, with no loss reports injected.

| Case | Mean send time | Per-run p99 range | Maximum range | Send calls/run |
| --- | ---: | ---: | ---: | ---: |
| Released 800 Mbps | 0.473 ms | 0.536–0.700 ms | 8.089–8.294 ms | 1,222 |
| Replacement automatic, healthy | 0.494 ms | 0.544–0.856 ms | 8.080–8.125 ms | 1,222 |
| Fixed 80 Mbps comparison | 10.429 ms | 11.002–11.005 ms | 86.833–87.477 ms | 7,974 |

Every run delivered all 46,042 datagrams and all 600 frames to an independent
receiver: zero missing packets across 414,378 datagrams. The replacement
stayed at 800 Mbps throughout the healthy runs. The small mean difference
(0.021 ms) and overlapping ranges are not an end-to-end latency claim or a
speedup. This measures host sender timing and loopback delivery only.

Build `udp_pacing_probe` at both revisions with
`cargo +1.98.1-x86_64-pc-windows-gnu build --release --locked -p butterpollo-windows --example udp_pacing_probe`,
retain the two executables, and run:

```powershell
python rust/tests/udp_pacing.py --before PATH/TO/before.exe --after PATH/TO/after.exe --output PATH/TO/results --runs 3
```

Use `--after-mode paced --after-rate 80 --comparison-rate 800` to reproduce
the rejected candidate's fixed-rate comparison. The harness rejects
incomplete delivery and retains executable hashes, arguments, statistics and
compressed receiver timestamps. Automatic probe SHA256:
`0c838526a3abaddf46c2d7ccaea9e412a5bc9eba2903e87b3de8fe0a0caae933`.

### Ordered sender handoff

The CPU-only, release-mode `sender_handoff_benchmark` compares identical
encrypted packetization with 20% FEC inline and through the actual
`Sender::spawn/submit/Slot::run` queue. Two alternating runs per path at each
rate measured 5,040 frames after 384 warmup frames. All packet counts,
digests and ordering matched; no submissions were a full frame period late.

| Rate | Inline mean | Sender mean | Difference | Sender queue p99 |
| --- | ---: | ---: | ---: | ---: |
| 60 FPS | 0.1862 ms | 0.2053 ms | 0.0191 ms | 0.0393 ms |
| 120 FPS | 0.0732 ms | 0.0931 ms | 0.0198 ms | 0.0352 ms |
| 240 FPS | 0.0303 ms | 0.0393 ms | 0.0090 ms | 0.0305 ms |

This separate measurement excludes sockets, capture, GPU work, thread
priority changes and client decoding. Do not add its difference to the UDP
result as though they were one end-to-end experiment. Run with:

```powershell
cargo test --release -p butterpollo --bin butterpollo sender_handoff_benchmark -- --ignored --nocapture --test-threads=1
```

### Regression models and earlier failures

The deterministic model keeps encoded bitrate at 40 Mbps/60 FPS with 20%
FEC, encryption, IPv4/IPv6 and two 8× keyframes. A 1 Gbps host feeding a
100 Mbps egress with a 16 KiB queue initially loses packets, then the
controller converges and the final three seconds have no losses. A 1 Gbps
egress keeps 800 Mbps pacing with no losses and normal frames below 2 ms.
This is **a queue model**, not a measurement of the reported Onn client,
its Ethernet speed or its decoder. Physical bottlenecks, competing traffic
and affected-client/end-to-end behavior remain unverified.

The first test attempt was blocked by sandbox networking (independent TCP
and UDP controls also failed). Granted network access made those controls
and the host/socket tests pass. Two real defects uncovered during follow-up
were fixed separately: Windows atomic replacement failed beyond legacy
path lengths, and USB/IP teardown could hang when its self-wake connection
was refused. Before/after regressions reproduce both and pass after fixing.
The integrated pre-controller workspace run passed 939 tests; only the
unchanged private-pipe and firewall-query tests still failed with
`E_ACCESSDENIED`. The independent firewall COM query also fails, and the
pipe trace points to `CreateFileW`, not a media test. These checks remain
enabled; their failure is not converted into a pass or an ignored test.

After integrating main `1af70632`, the final local workspace run passed
967 tests, with the same two access-denied failures and 58 existing/manual
checks ignored (including the separately run handoff benchmark). All 423
core tests and 222 ordinary host tests passed. Strict all-target Clippy,
formatting and the Svelte check passed. Four subcases of the release-helper
reuse test could not start Git Bash: `NtCreateDirectoryObject` failed with
`0xC0000022` while creating its MSYS named object. The other four release
test methods passed. No test was weakened to accommodate these restrictions.

The first [GitHub Windows run](https://github.com/RamazanKara/Rubylight/actions/runs/38534394016)
passed the release-tooling checks but failed the reconnect fixture's requirement
to receive more than ten synthetic frames in 750 ms. This fixture bypasses the
pacing controller and sends directly; the log does not establish why that
throughput assertion failed. The test now records a successful live-session
video send after the production control worker's one-second inactivity
checkpoint, waits for teardown, then verifies the unchanged two-second control
timeout, failure reason and reconnect. It still requires received video to
advance. The final local host suite passes all 222 ordinary tests, five extra
focused repetitions pass, and formatting and strict Clippy pass.

The unchanged private-pipe and firewall-query tests subsequently passed when
run outside the local sandbox with approval. The sandbox failures therefore
do not require a product change or a relaxed security/test check. Their logs
are saved as `pipe-outside-sandbox.log` and `firewall-outside-sandbox.log`.

The final [Windows CI run](https://github.com/RamazanKara/Rubylight/actions/runs/38538959880)
on `dff0623ee0ebef912aab703daec226f7aa3df208` passed **969 workspace tests,
zero failures**, with 58 hardware/manual checks ignored. The reconnect
regression, private-pipe and firewall checks all passed. Formatting, strict
Clippy, console and release-tooling checks, release compilation and installer
packaging also passed. The build produced the Windows package artifact;
no tag, public release or local installation was performed during that validation.

Raw timing, benchmark hashes, test logs and controls are retained under
`C:\Users\ramaz\Documents\Codex\2026-10-13\on-ruby-lite-or-ruby-lite\outputs\pacing-followup`.

### Conservative startup before 2.2.19

**Release decision:** preserve the existing fast route-based policy as the
default. `host_adaptive_pacing` is experimental and defaults to `false`.
A suitable fixed rate is the simpler choice for a known constrained path;
adaptation is an option when manual tuning is inconvenient. The comparisons
below do not justify adding adaptive startup delay to every healthy connection.

The fast-start candidate still exposed a constrained client to an initial
800 Mbps burst. The optional controller starts before the first packet at twice
the encoder bitrate or 1.25 times packetized wire demand, whichever is higher,
within the route ceiling. It probes upward by 12.5% only with busy video,
recent standard control pings and stable nonzero RTT. Each step waits at least
500 ms and twice RTT plus 100 ms; a rising RTT extends an outstanding probe's
confirmation period. The first validated probe loss rolls back promptly and
enters slower recovery. Every reconnect starts conservatively rather than
reusing a stale fast-path estimate.

A 25% startup probe was also rejected before release. In the deterministic
100 Mbps egress / 16 KiB queue fixture it preserved the first picture but
caused 2, 3 or 6 unrecoverable frames during later probes, depending on feedback
delay. A 25% step can imply 20% packet loss above a previously sustainable
rate, exceeding the roughly 16.7% total-packet loss fraction covered by 20%
parity even under ideal distribution. Halving the step leaves more repair
margin; it is a probing heuristic, not a capacity measurement.

The updated production-controller model runs 960 frames over 16 seconds,
with 40 Mbps encoded video at 60 FPS, 20% FEC, encryption, IPv4/IPv6 and
8x keyframes at the start and five seconds. Feedback is delayed by 2, 20 or
80 ms rather than delivered immediately. No startup or probe interval is
excluded from the unrecoverable-frame count.

| Egress | Feedback delay | Packet losses | Unrecoverable frames | Final pacing |
| --- | ---: | ---: | ---: | ---: |
| 100 Mbps | 2 ms | 2 | 0 | 90 Mbps |
| 100 Mbps | 20 ms | 3 | 0 | 90 Mbps |
| 100 Mbps | 80 ms | 6 | 0 | 90 Mbps |
| 1 Gbps | 2 / 20 / 80 ms | 0 | 0 | 800 Mbps |

Counts match for IPv4 and IPv6. All initial keyframes arrive intact, completing
in about 87.9 ms (IPv4) or 89.1 ms (IPv6) on the modeled slow path. The model
records remaining probe losses rather than hiding them; its 20% FEC repairs
them. These results do not guarantee the same recovery with lower FEC, other
queues, competing traffic or the affected Onn device.

The matched comparison also runs fixed 800 Mbps and fixed 80 Mbps with the
same packetizer, bitrate, FPS, FEC, queue and feedback delays: 36 cases in all.
The table spans IPv4/IPv6 and all three feedback delays. Delivery is measured
from each frame's scheduled start, including backlog; incomplete frames are
counted as failures, not silently treated as low-latency deliveries.

| Policy | Healthy first IDR | Healthy first-two-second mean | 100 Mbps steady mean after 10 s | 100 Mbps losses over all 960 frames |
| --- | ---: | ---: | ---: | --- |
| Fixed 800 Mbps | 9.27–9.47 ms | 1.15–1.17 ms | No complete frames | All 960 frames unrecoverable |
| Fixed 80 Mbps | 87.69–88.85 ms | 15.27–15.68 ms | 10.67–10.82 ms | None |
| Adaptive | 87.69–88.85 ms | 13.60–13.98 ms | 9.45–9.58 ms | 2–6 packets, all repaired |

All three policies deliver every frame without loss on the healthy modeled
1 Gbps path. Adaptation eventually matches the fast policy there, but its
opening picture is about 78–79 ms later. On the modeled slow path, a suitable
fixed rate solves the failure without probing; adaptation saves only about
1.22 ms of steady delivery time. These are deterministic queue-model results,
not physical-network or end-to-end measurements.

Twelve real Windows UDP loopback runs separately measured sender timing:
three 20-second repetitions per policy, each with 1,200 frames at 60 FPS,
50 Mbps UDP payload approximating a 40 Mbps encoder plus 20% FEC/framing,
and 8x keyframes every five seconds. All 1,105,008 packets and 14,400 frames
arrived. Automatic runs use synthetic standard pings and stable 2 ms RTT;
loopback does not test congestion feedback or client capacity. Values below
are means across the three runs, in milliseconds of frame-send duration.

| Policy | First IDR | First-two-second mean | Steady mean after 15 s | Time to 800 Mbps |
| --- | ---: | ---: | ---: | ---: |
| Fixed 800 Mbps | 8.145 | 0.529 | 0.530 | Immediate |
| Fixed 80 Mbps | 86.887 | 10.874 | 10.448 | Not applicable |
| Adaptive, synthetic feedback | 86.969 | 9.238 | 0.535 | 10.17–10.20 s |
| Adaptive, no feedback | 87.028 | 10.875 | 10.456 | Never in the 20 s run |

The first frame and opening seconds are reported separately so the long-term
mean cannot hide startup cost. Sender duration excludes time waiting before
a frame is submitted, decode and display; it must not be read as picture age.
No-feedback runs remain at their 80 Mbps initial rate. The benchmark and model
support keeping adaptation optional, not a claim that it fixes the Onn report.

Regression tests also cover fresh and repeated sessions, no/invalid control
activity, changed FEC floors, stale and duplicate loss reports, a pending
probe loss racing an FEC update, changing route ceilings, higher RTT,
client-owned bitrate/FPS and manual/PyroWave bypasses. A confirmed low route
ceiling cannot jump to a higher one without fresh activity. An FEC change
cannot erase a failed probe and resume fast probing.

The final local workspace passed 983 tests with 58 existing hardware/manual
checks ignored, including 437 core tests, 28 pacing tests and 222 host tests.
Formatting, strict all-target Clippy, the Svelte console check and all five
release-tooling tests passed. Independent code and documentation reviews
found no remaining actionable regressions after the corrections above.
The policy matrix covers the disabled default, explicit opt-in, manual limits,
host bitrate adaptation, PyroWave, unknown codecs, resetting defaults and
per-device/app overrides. No capture, display change, installed service,
controller driver, game input or streaming session was used. Evidence is
retained in `outputs/startup-pacing`, including `matched-policy-model.json`,
`benchmark-summary.json` and `workspace-opt-in-final.log`; raw UDP records
are under `D:\CodexArtifacts\rubylight-startup-pacing\loopback`.

### RDP transition checks before 2.2.19

The [latest issue #18 report](https://github.com/RamazanKara/Rubylight/issues/18#issuecomment-6164080730)
confirms that headless display handover still works, but describes a stream
accepted after RDP disconnection and terminated during Windows arbitration.
The existing code already queries the host process's own Windows session ID;
the report does not establish a wrong-ID lookup. It did expose a timing gap:
the session check ran before display/audio/app preparation, while the accepted
launch log appeared only after that potentially lengthy work.

The host now rechecks after preparation and immediately before RTSP playback.
A newly prepared app stays locally owned until acceptance, so a late refusal
runs its teardown and undo commands instead of retaining it as the current
app. Pending display and audio preparations are released. A resumed game or
an app already accepted by HTTP retains its existing resumable lifecycle;
rejecting one pending PLAY does not stop another client's stream.

The Windows classifier still supports genuine console sign-in and lock-screen
sessions. New diagnostics record the host and console IDs, connection state,
protocol, acceptance reason and launch phase. They are needed to distinguish
a transition during preparation from an unexpected Windows classification;
no observed bad classification tuple was supplied with the report.

Explicit Return to console additionally permits a signed-in destination only
when it is disconnected and Windows user tokens prove both sessions belong
to the same user. Its state and the console ID are rechecked after those
identity reads. Active destinations, different users, unavailable identities
and changed IDs remain blocked. This exception normally requires the installed
SYSTEM service; portable hosts without token-query access retain the existing
empty-console path. No session is moved automatically.

Regression coverage injects Windows state changes into real HTTP/RTSP paths
and uses fake display/audio owners. It also exercises a hidden preparation
command and its undo marker. No physical RDP handover, live console transfer,
display change or installed-host restart was performed. The affected host
still needs to confirm recovery, and Windows can still switch sessions after
playback has begun.

The combined local workspace passed **991 tests, zero failures**, with 58
existing hardware/manual checks ignored: 225 host, 437 core, 275 Windows,
50 setup and four credential/Playnite/Vulkan tests. The new authorization and
Terminate regression also passed in that full run. Evidence is retained in
`outputs/issue-18-session-gate`, including `workspace-final.log`,
`host-session-transition-tests.log` and `web-check.log`.
Formatting, strict all-target Clippy and all five release-tooling checks also
passed. Independent final code and documentation review found no actionable
regressions; it did not perform a live Windows session transfer.

The first combined Windows CI run failed the new undo-marker fixture before
the session gate: its administrator runner had no linked non-elevated token
(`0x80070520`). The fixture now uses the same existing-runner-context selection
as the project's process lifecycle tests. The real prep/undo assertion remains,
and no production permission policy or session check was relaxed.

## October 14: frame rates above 60

A user reported that the stream would not go above 60 fps whatever frame rate
they set, and an earlier report had an RX 9070 XT stuck at 60. The release
check streamed only 720p60 at 20 Mbps, so higher rates were never tested there.

**Machine:** AMD Radeon RX 7900 XT (driver 32.0.32015.2008), Ryzen 7 5800X3D,
Windows 26200, one 5120x1440 monitor at 240 Hz. **Host:** the released 2.3.0
package, isolated profile on ports 48518-48544 (`rust/release/e2e.py`), WGC
capture of the physical monitor (no virtual display, no display change),
AMF defaults, `minimum_fps_target = 20` (the default; the release check sets
it to the stream rate). **Receiver:** the independent moonlight-common-c
fixture with D3D11VA decoding on the same GPU. `motion_probe` moves a
128-pixel strip at twice the stream rate. 12 seconds per stream.

Every stream delivered its requested rate. The encoder took about 3 ms per
frame at every rate and bitrate; capture supplied a fresh picture for every
frame sent (fresh captures per second, from the host's 5-second timing
lines). The strip is easy to compress, so the bitrate set is the encoder's
target, not what it sent: 5-88 Mbps went out, the most for HEVC at 240 fps.

| Codec | FPS | Mbps set | Mbps sent | Delivered fps | Host sent fps | Fresh captures/s | Encode mean / p99 ms | Host ms | Arrival p99 / max ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| AV1 | 60 | 20 | 5.5 | 60.0 | 60.0 | 60.5 | 2.84 / 3.51 | 2.84 | 18.4 / 20.7 |
| AV1 | 60 | 80 | 6.0 | 60.0 | 60.0 | 60.6 | 2.79 / 3.06 | 2.79 | 19.4 / 21.5 |
| AV1 | 60 | 150 | 5.3 | 60.0 | 60.0 | 60.5 | 2.94 / 3.46 | 2.94 | 17.8 / 20.5 |
| AV1 | 60 | 300 | 5.8 | 60.0 | 60.1 | 60.5 | 2.86 / 3.50 | 2.87 | 21.4 / 22.6 |
| AV1 | 60 | 500 | 5.6 | 60.0 | 60.0 | 60.5 | 2.97 / 3.43 | 2.98 | 18.7 / 24.1 |
| AV1 | 120 | 20 | 12.1 | 120.0 | 119.9 | 120.5 | 2.81 / 3.14 | 2.81 | 10.7 / 12.8 |
| AV1 | 120 | 80 | 10.9 | 120.0 | 120.1 | 120.7 | 2.83 / 3.40 | 2.83 | 10.1 / 12.8 |
| AV1 | 120 | 150 | 13.2 | 120.0 | 120.0 | 120.4 | 2.79 / 3.50 | 2.79 | 10.0 / 13.2 |
| AV1 | 120 | 300 | 13.5 | 120.0 | 119.9 | 120.6 | 2.80 / 3.31 | 2.82 | 11.1 / 13.6 |
| AV1 | 120 | 500 | 15.0 | 120.0 | 120.0 | 120.5 | 2.82 / 3.35 | 2.83 | 11.7 / 13.3 |
| AV1 | 240 | 20 | 21.0 | 240.1 | 240.1 | 240.4 | 2.65 / 3.92 | 2.65 | 5.9 / 7.0 |
| AV1 | 240 | 80 | 39.9 | 239.1 | 239.1 | 239.0 | 2.74 / 4.08 | 2.74 | 6.0 / 12.0 |
| AV1 | 240 | 150 | 40.0 | 240.0 | 240.1 | 240.5 | 2.71 / 3.96 | 2.72 | 5.9 / 8.3 |
| AV1 | 240 | 300 | 40.3 | 239.7 | 239.8 | 239.9 | 2.77 / 4.07 | 2.78 | 6.0 / 9.2 |
| AV1 | 240 | 500 | 41.7 | 240.0 | 239.9 | 240.5 | 2.75 / 3.99 | 2.77 | 6.0 / 8.2 |
| HEVC | 60 | 20 | 13.3 | 60.0 | 60.1 | 60.5 | 3.18 / 3.73 | 3.18 | 18.2 / 18.5 |
| HEVC | 60 | 80 | 15.4 | 60.0 | 60.1 | 60.6 | 3.21 / 3.63 | 3.21 | 18.3 / 18.9 |
| HEVC | 60 | 150 | 14.9 | 60.0 | 60.0 | 60.5 | 3.16 / 3.54 | 3.16 | 18.6 / 23.0 |
| HEVC | 60 | 300 | 15.5 | 60.0 | 60.0 | 60.5 | 3.25 / 3.73 | 3.26 | 18.3 / 24.0 |
| HEVC | 60 | 500 | 17.0 | 60.0 | 60.1 | 60.7 | 3.16 / 3.59 | 3.17 | 21.9 / 23.2 |
| HEVC | 120 | 20 | 21.0 | 120.0 | 120.0 | 120.6 | 3.10 / 3.58 | 3.10 | 10.9 / 13.0 |
| HEVC | 120 | 80 | 32.5 | 120.0 | 120.0 | 120.7 | 3.12 / 3.64 | 3.12 | 10.8 / 12.7 |
| HEVC | 120 | 150 | 34.9 | 120.0 | 120.0 | 120.6 | 3.17 / 3.71 | 3.17 | 10.5 / 11.2 |
| HEVC | 120 | 300 | 33.2 | 119.9 | 120.0 | 120.5 | 3.13 / 3.61 | 3.14 | 10.4 / 12.3 |
| HEVC | 120 | 500 | 35.6 | 120.0 | 120.1 | 120.4 | 3.14 / 3.64 | 3.15 | 9.8 / 11.7 |
| HEVC | 240 | 20 | 20.4 | 240.0 | 240.0 | 240.1 | 3.15 / 4.04 | 3.15 | 6.1 / 6.9 |
| HEVC | 240 | 80 | 76.7 | 239.3 | 239.3 | 239.9 | 3.18 / 4.24 | 3.18 | 6.1 / 7.5 |
| HEVC | 240 | 150 | 84.1 | 239.6 | 239.6 | 240.0 | 3.18 / 4.17 | 3.18 | 6.1 / 15.3 |
| HEVC | 240 | 300 | 85.6 | 239.4 | 238.8 | 239.1 | 3.15 / 4.05 | 3.16 | 6.4 / 7.7 |
| HEVC | 240 | 500 | 87.7 | 236.8 | 236.6 | 238.1 | 3.16 / 4.05 | 3.17 | 6.8 / 12.4 |

At 240 fps the receiver counted 205-217 distinct pictures a second: the probe
itself presented 233 a second on the 240 Hz monitor, below the 480 it was
asked for. The release check's motion rule (90% distinct pictures) fails most
of those runs for that reason; the host sent 237-240 fresh frames a second.

The strip leaves the encoder little to spend. With a 512-pixel strip of fresh
per-pixel noise (`e2e.py --noise-height 512`), the encoder used the bitrate
set (the sent rate includes 20% FEC):

| Codec | FPS | Mbps set | Mbps sent | Delivered fps | Host sent fps | Fresh captures/s | Encode mean / p99 ms | Decode ms | Arrival p99 / max ms | Keyframe requests |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| AV1 | 120 | 500 | 470.2 | 120.0 | 120.0 | 120.4 | 4.29 / 7.62 | 5.9 | 13.8 / 16.9 | 1 |
| HEVC | 120 | 500 | 543.7 | 119.9 | 119.9 | 120.3 | 4.04 / 4.46 | 5.0 | 12.1 / 20.0 | 1 |
| AV1 | 240 | 500 | 497.2 | 142.9 | 224.1 | 224.5 | 4.15 / 6.16 | 6.8 | 82.6 / 103.8 | 52 |
| HEVC | 240 | 500 | 535.2 | 238.4 | 238.3 | 238.7 | 3.68 / 4.31 | 3.6 | 5.8 / 16.5 | 1 |
| AV1 | 240 | 150 | 160.5 | 238.5 | 238.6 | 238.9 | 3.08 / 4.57 | 4.1 | 6.8 / 16.9 | 1 |
| HEVC | 240 | 150 | 159.0 | 239.6 | 239.6 | 240.3 | 3.27 / 3.76 | 2.6 | 5.2 / 12.7 | 1 |

The one shortfall is AV1 at 240 fps and 500 Mbps: a 4.15 ms mean encode
against a 4.17 ms period held the host to 224 fps, and the receiver, decoding
on the same GPU at 6.8 ms a frame, lost frames and asked for 52 keyframes.
HEVC sustained 240 fps at the same bitrate, and AV1 at 150 Mbps.

The release check's receiver decodes in software, which held a 1440p120 HEVC
stream to 65 fps (14 ms a frame) while the host sent 120. Its new `hevc-120`
case (2560x1440, 120 fps, 80 Mbps) decodes with D3D11VA: 119.97 fps, arrival
p99 12.0 ms. A second run with the CPU at 100% from other work failed on the
fixture: `motion_probe` presented only 184-191 pictures a second.

### What can hold a stream at 60

None of the code paths clamps the rate to 60. A stream runs at the lowest of
the stream rate, the captured display's refresh and the game's own rate:

| Path | Default | Effect |
| --- | --- | --- |
| `limit_framerate` (Use the launch frame rate) | on in 2.3.0, off since (below) | On, the stream runs at the rate of the launch request (`mode=WxHxFPS`), not the stream request (`maxFPS`). Artemis's Warp frame pacing launches at the display rate and asks for 2x or 4x. Vibepollo, with the same setting on, only uses the launch rate for the encoder's frame timestamps and captures at the requested rate. |
| `virtual_display_mode` | per client on Windows 11, disabled on Windows 10 | On the physical display the host asks for the stream rate, but keeps the current refresh, or the highest at the chosen resolution, when the monitor lacks it. A 60 Hz monitor gives 60. |
| Virtual display refresh | twice the launch rate (`frame_limiter_auto_virtual_framegen = legacy`) | Applied after creation; when Windows keeps another mode the log says "virtual display mode not applied" and capture runs at that mode. |
| Frame limiter, Automatic | on with a virtual display | Caps the game at the launch rate (RTSS, or NVIDIA's driver limit without RTSS). |
| Phase lock | only with the Rubylight client's 0x5530 reports | Locks only when a frame lasts a whole number of client refreshes (within 2%); a 120 fps stream to a 60 Hz client keeps 120. |
| WGC `MinUpdateInterval` | set to 0 | If Windows refuses it, the log warns and the 16 ms default (50-57 updates a second) applies. |
| AMF | frame rate = stream rate; bitrate, peak, VBV read back as set (399 Mbps target at 500 Mbps requested) | No level or profile limit was seen. |

The launch-rate path, measured with the receiver launching at one rate and
asking for twice that in the stream request (HEVC, 2560x1440, 80 Mbps):

| Host | Launch, then stream request | Use the launch frame rate | Delivered fps | Console `fps` / `requested_fps` |
| --- | --- | --- | ---: | --- |
| 2.3.0 | 60, then 120 | on (default) | 60.0 | 120 / – |
| 2.3.0 | 60, then 120 | off | 120.1 | 120 / – |
| This branch | 60, then 120 | on (default) | 60.0 | 60 / 120 |
| 2.3.0 | 120, then 240 | on (default) | 120.0 | 240 / – |
| 2.3.0 | 120, then 240 | off | 240.1 | 240 / – |

With the setting on, 2.3.0's console showed the stream request's rate as the
target (120) while 60 frames a second went out, which reads as a host that
cannot keep up. The console now shows the encoded rate and the requested one.

### Streams at the requested rate

Since then `limit_framerate` is off by default: the stream runs at the
stream request's rate, as in Vibepollo. The client's bitrate stays the budget
(Vibepollo multiplies it for Warp), so each frame gets half or a quarter of
it. When the launch prepared a virtual display for its own rate, the display
is kept and its refresh raised where the stream needs more (twice the stream
rate by default), and the frame limiter follows the stream; a physical
display, or a virtual one Windows will not raise, is prepared again at the
stream's rate. The same machine and fixture as above, HEVC unless noted,
2560x1440 at 80 Mbps (62,988 Kbps encoded in every run), physical monitor,
`minimum_fps_target = 20`, CPU 57-65% busy with other work:

| Host | Launch, then stream request | Delivered fps | Picture age p95 |
| --- | --- | ---: | ---: |
| This change | 60, then 120 | 119.9 | 13.9 ms |
| This change, AV1 | 60, then 120 | 119.8 | 22.9 ms |
| This change | 120, then 240 | 239.6 | 17.6 ms |
| This change | 120, then 120 | 119.6 | 21.7 ms |
| 2.3.0 | 120, then 120 | 120.0 | 19.1 ms |
| This change, `limit_framerate = true` | 60, then 120 | 60.1 | 21.9 ms |

The release check's new `hevc-warp` case (launched at 60, asking for 120)
passed with this change (120.05 fps) and failed with 2.3.0 (60.0 fps). The
virtual display path was not measured: the driver accepts only SYSTEM, and
running a test host as SYSTEM needs a one-shot SYSTEM task that was not
created here.

## October 14: keyframe loss on a slow port

An Artemis client on an Onn 4K Pro (100 Mbps Ethernet) lost 94-99% of its
frames. Besides the Warp bitrate multiplication fixed before this, a lost
keyframe was answered with another full keyframe at the same burst rate.

**Bottleneck.** `udp_shaper` (`rust/windows/examples/udp_shaper.rs`) relays
RTSP, control and audio from 127.0.0.2 to the isolated host on 127.0.0.1
unchanged, and drains the video port toward the client at 100 Mbps through a
drop-tail queue of 16, 64 or 256 KiB, counting 66 bytes of Ethernet, IP and
UDP framing per datagram. `rust/release/bottleneck.py` streams through it
like `e2e.py`. The shaper's self-test (`udp_shaper --selftest`) feeds paced
80 and 200 Mbps and 250 KB bursts through each queue size over loopback and
replays the sender's own send times through the same queue model. On an idle
machine 80 Mbps passed without a drop and 200 Mbps came out at 99.86-99.97
Mbps with a median spacing of 120.1-120.4 us (120.5 us expected). With the
link thread at game priority and other builds holding the CPU at 97-99%,
three runs forwarded within 1% of the model in every case (exactly, in most),
spacing 119.2-120.2 us; no datagram was lost outside the model. The stream
runs below used the shaper before that priority; the six repeated at the end
of this section used it.

**Streams.** RX 7900 XT, AMF, 2560x1440 at 60 fps with `motion_probe`'s
128-pixel strip, 20 seconds, client bitrate 40 or 60 Mbps (31 or 47 Mbps
encoded), AV1 and HEVC. The moonlight-common-c receiver declares reference
frame invalidation, as Moonlight's hardware decoders do, and decodes in
software unless noted. "Whole" counts the frames the receiver got complete;
Moonlight passes no frame to its decoder while it waits for a keyframe. The
host's frame count is its last API sample, half a second before the end, so
the share can exceed 100%. Picture age runs from the strip's QPC stamp to
decode on the same machine. The candidate policies ran in a build of this
branch with the mode selectable; "released" is the policy 2.3.0 ships.

Each cell is the range over the four codec and bitrate combinations, with
the keyframe requests the host counted:

| Policy | 16 KiB | 64 KiB | 256 KiB | No bottleneck |
| --- | --- | --- | --- | --- |
| Released (800 Mbps wired pacing) | 0% (297-308 req) | 0% (305-308 req) | 100% (1 req) | 95-100% (1-2 req) |
| 2.2.1 | 0% (303-305 req) | 0% (303-307 req) | 100% (1 req) | – |
| (a) forced keyframes at most every 500 ms | 0% (65-99 req) | 0% (27-30 req) | 81-100% (1-80 req) | – |
| (b) keyframes alone at the Wi-Fi rate | 2-66% (7-162 req) | 95-100% (1-3 req) | 99-100% (1 req) | – |
| (c) `amd_max_frame_size = 2` | 0% (307-308 req) | 0% (299-305 req) | 84-100% (1-3 req) | – |
| (d) `host_adaptive_pacing` (opt-in) | 100% (1-4 req) | 100% (1 req) | 100% (1 req) | 100% (1 req) |
| `pacing_max_bitrate_kbps = 80000` | 100% (1 req) | 100% (1 req) | 100% (1 req) | 100% (1 req) |
| Wi-Fi rate after a lost keyframe (this change) | 100% (3 req) | 100% (3 req) | 100% (1 req) | 100% (1 req) |

With the released policy every frame was a keyframe: the host sent about 600
frames in 20 seconds, the receiver asked for a new keyframe after each, and
the shaper dropped 53-82% of all video packets. Limiting how often keyframes
are forced (a) spaced the requests out but every keyframe was still lost. A
smaller keyframe (c) still left at 800 Mbps. Pacing keyframes alone (b) let
them through the 64 KiB queue, but ordinary frames overflowed 16 KiB. The
256 KiB queue held these keyframes. The 95% control and the 84% cell of (c)
are HEVC runs where the receiver's software decoder fell behind (10-13 ms a
frame) and lost frames with no shaper drop; in the 81% run of (a) the 256 KiB
queue overflowed (836 packets) and deferred keyframes stretched each recovery.

**This change** keeps 800 Mbps until a keyframe is lost: the client reports
it could not repair a keyframe (FEC status), or asks again for the keyframe
it was just sent. From then on the stream is paced like a confirmed Wi-Fi
route: twice the encoder bitrate, at least 1.25 times packetized demand
(62 Mbps for the 31 Mbps streams, 94 for 47). In the final build, with
the setting at its default, pacing slowed 38-49 ms after each stream started,
by either signal, and the next paced keyframe arrived; the host counted three
keyframe requests in all (its own first one included):

| Stream | Queue | Whole / sent | IDR requests | Shaper drops | Steady fps | Picture age p95 / max ms | Pacing Mbps |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| AV1 40 Mbps | 16 KiB | 1162 / 1164 | 3 | 223 of 12197 | 60.0 | 17.6 / 32.5 | 800, then 62 |
| AV1 40 Mbps | 64 KiB | 1172 / 1172 | 3 | 185 of 15055 | 60.0 | 29.7 / 46.7 | 800, then 62 |
| HEVC 60 Mbps | 16 KiB | 1179 / 1176 | 3 | 365 of 30924 | 60.0 | 21.6 / 37.7 | 800, then 94 |
| HEVC 60 Mbps | 64 KiB | 1179 / 1178 | 3 | 277 of 34210 | 60.0 | 278.6 / 408.8 | 800, then 94 |
| HEVC 40 Mbps, no RFI | 64 KiB | 1174 / 1172 | 3 | 227 of 39079 | 60.0 | 34.8 / 39.8 | 800, then 62 |
| AV1 40 Mbps, no RFI | 16 KiB | 1181 / 1175 | 3 | 222 of 11876 | 60.0 | 15.1 / 27.8 | 800, then 62 |
| AV1 60 Mbps at 120 fps, D3D11VA | 64 KiB | 1600 / 1585 | 3 | 150 of 23902 | 120.2 | 32.2 / 131.6 | 800, then 94 |
| AV1 80 Mbps | 64 KiB | 1169 / 1174 | 3 | 210 of 18497 | 60.0 | 43.9 / 114.6 | 800, then 125 |
| AV1 40 Mbps, setting off | 64 KiB | 0 / 601 | 302 | 28215 of 52983 | – | – | 800 |

(The 279 ms HEVC picture age is the software decoder again; it decoded every frame.)

**Game-sized frames.** A 512-pixel noise strip made the encoder spend its
budget (100,000 packets in 20 s for AV1 40 Mbps instead of 15,000). HEVC
60 Mbps through 64 KiB: 1185 of 1184 frames whole with the change (2 requests),
0 of 596 with it off (300 requests). AV1 40 Mbps through 256 KiB: whole with
the setting on or off; the keyframe still fit.

**Healthy path.** Without a bottleneck the change never engaged (one
request, 800 Mbps throughout). Receiver decoding on D3D11VA, picture age p95:

| Stream | Released policy / this change | `pacing_max_bitrate_kbps = 80000` | `host_adaptive_pacing` |
| --- | ---: | ---: | ---: |
| AV1 40 Mbps | 19.5 ms (this change) | 40.1 ms | 79.1 ms |
| HEVC 60 Mbps | 21.3 and 22.8 ms (2.3.0), 20.1 and 25.9 ms (this change) | 53.6 ms | 20.3 ms |

One more HEVC run of this change measured 141.7 ms; it sent at 800 Mbps
like the released host, and the repeats above did not reproduce it.

**Repeat with the released package**, the prioritized shaper and D3D11VA
decoding:

| Stream | Queue | 2.3.0: whole / sent, IDR requests | This change: whole / sent, IDR requests, picture age p95 |
| --- | ---: | --- | --- |
| AV1 40 Mbps | 16 KiB | 0 / 602, 304 | 1183 / 1175, 3, 23.9 ms |
| AV1 40 Mbps | 64 KiB | 0 / 590, 294 | 1187 / 1175, 2, 23.2 ms |
| HEVC 60 Mbps | 64 KiB | 0 / 595, 299 | 1192 / 1198, 3, 26.0 ms |

**2.2.1** behaved as 2.3.0 on every case (first table): the same 800 Mbps
wired policy and keyframe recovery.

**Not measured:** the Onn 4K Pro and Artemis themselves, a real switch's
queue with other traffic, and Wi-Fi. A client whose bitrate is close to its
port's speed (80 Mbps on 100) is paced above the port after the change (125
Mbps); it recovered here, with the strip's small frames.

## October 15: PyroWave SDK patches from Punktfunk

Punktfunk (MIT OR Apache-2.0) builds PyroWave with local patches. Three of
them concern the encoder the host uses, and each was checked against the SDK
the host pins (`502a3b52`):

| Punktfunk patch | Its measurement | Pinned SDK and host | Result |
| --- | --- | --- | --- |
| 0004, encoder buffer pool | Submit and fence wait at 5120×1440 from about 15 ms to 1 ms (RTX 4090) | `pyrowave_encoder_encode_gpu_inner` keeps the meta and bitstream buffers per frame context and reallocates only for a larger size. The patch targets the older `encode_gpu_synchronous_inner`, as Vibepollo's buffer-pool patch did for `186f0393`. | Not needed |
| 0005, global-priority queue | Tighter encode tail beside a GPU-bound game (Linux) | `pyrowave_create_device_by_compat` takes a priority, and Granite falls back from REALTIME to HIGH to MEDIUM. The host asks for HIGH; the probe logs `requested=512 granted=512` on this card. | Not needed |
| 0010, cached plane views | 0.15–0.25 ms of CPU per frame (RTX 5070 Ti) | `pyrowave_encoder_encode_gpu` still wraps three images and creates three image views on every call, and the host passes the same three planes every frame. | Ported and measured, not shipped |

The port keys a plane on an id the device gives each `pyrowave_image`, not on
its `VkImage` handle as Punktfunk's does. A new capture format makes the host
destroy its planes and create new ones, and `pyrowave_image_destroy` lets the
old images be freed first. The ignored test
`pyrowave_encoder_follows_rebuilt_planes` alternates two HDR source formats:
in its seven rebuilds the driver gave 7 to 9 of the 21 new planes the handle
of a destroyed one. No rebuild returned all three previous handles in order,
which a handle-only key needs before it binds views of freed images, but
nothing rules it out. Every frame decoded to its own picture (at least 40 dB
per plane at 1 Gbps) and to the same pixels as the first encode of that
picture, with and without the patch. PyroWave's own C API tests
(`pyrowave-c-test`, `pyrowave-c-interop-test`: D3D11, D3D12 and Vulkan
interop) pass with both builds, and `pyrowave_decoded_end_to_end` passes with
the patch.

Before and after are the same commit's SDK, built by
`rust/tools/build_pyrowave.sh` with and without the new patch, under one
probe build: `examples/performance.rs --codec pyrowave --synthetic 16 --fps
120 --bitrate 1500000 --paced`, six seconds a run, three alternating runs a
cell, on the RX 7900 XT and a Ryzen 7 5800X3D. Each run waited for an idle
installed host, no isolated test host or end-to-end run, 3D load under 15%
and free commit memory; other agents were compiling at times. The encode call
covers conversion, encoding, the fence wait and packetization. Submit is the
`pyrowave_encoder_encode_gpu` call alone, timed in that probe build only, and
includes the first two seconds of paced warm-up.

| Picture | Size | Encode call mean / p95, before → after (ms) | Submit mean / p95, before → after (ms) |
| --- | --- | --- | --- |
| SDR 4:2:0 | 1920×1080 | 1.20 / 1.40 → 1.05 / 1.27 | 0.173 / 0.261 → 0.177 / 0.280 |
| SDR 4:2:0 | 2560×1440 | 1.22 / 1.48 → 1.27 / 1.52 | 0.175 / 0.255 → 0.175 / 0.298 |
| SDR 4:2:0 | 5120×1440 | 1.58 / 1.81 → 1.62 / 1.88 | 0.159 / 0.247 → 0.152 / 0.222 |
| HDR 4:4:4 | 1920×1080 | 1.14 / 1.33 → 1.17 / 1.35 | 0.183 / 0.323 → 0.164 / 0.255 |
| HDR 4:4:4 | 2560×1440 | 1.57 / 1.81 → 1.50 / 1.75 | 0.180 / 0.257 → 0.178 / 0.262 |
| HDR 4:4:4 | 5120×1440 | 1.86 / 2.16 → 1.68 / 1.90 | 0.170 / 0.308 → 0.185 / 0.308 |

Frames filled the budget, about 1.57 MB, except 1080p HDR 4:4:4 at 1.35 MB.
Run means of the encode call moved by 0.3 to 0.5 ms between runs of the same
build, so no cell differs beyond that. To resolve the submit call, six
alternating unpaced pairs at 1920×1080, about 9,000 to 11,000 frames a run:

| Picture | Submit mean / p95, before → after (ms) | Pairs lower after |
| --- | --- | --- |
| SDR 4:2:0 | 0.185 / 0.283 → 0.187 / 0.288 | 4 of 6, mean difference +0.002 ms |
| HDR 4:4:4 | 0.196 / 0.299 → 0.176 / 0.275 | 5 of 6, by 0.022 to 0.027 ms |

On this machine the per-frame views cost at most about 0.02 ms of a 0.18 ms
submit call, far from the 0.15–0.25 ms Punktfunk measured on NVIDIA, and the
buffer pool has nothing left to win: the whole paced encode at 5120×1440 HDR
4:4:4 takes 1.7 to 1.9 ms. The SDK therefore stays without the patch: a new
cache in the encoder for a gain inside run-to-run spread is not worth the risk.
The port stays on the `claude/pyrowave-fixes` branch, and the rebuild test
stays on main, where it passes with the shipped SDK.

## October 15: where host latency goes

Moonlight's host latency from Rubylight runs from the claim to packetization
(native AMF: from the encode call's entry; the separate sender stops at its
dequeue). It leaves out the capture and the wait before the claim, and the
send after packetization, so it cannot be set beside a host that times from
Windows' present. Each new picture now records these points on the host's
monotonic clock:

| Point | Taken when |
| --- | --- |
| Present | The capture's present stamp: Desktop Duplication's `LastPresentTime` (or the pointer's update), WGC's `SystemRelativeTime`. A stamp in the future or more than two seconds old is replaced by the capture's arrival, as the pacing clock has always done. |
| Acquired | The host submitted its own copy of the picture, or took a WGC helper texture to read in place. |
| Published | The capture worker published the picture to the stream threads. |
| Claimed | The stream thread took the picture to encode. |
| Submitted | AMF accepted the converted picture (`SubmitInput` returned). Encoders that do not report it put their conversion in the encode stage. |
| Encoded | The encoded frame was in the host's hands (`QueryOutput` returned it). |
| Packetized | Packetization began. |
| First sent | The first send call for the frame returned. |
| Last sent | The last send call returned. |

The stages between them are capture, hold, claim, convert, encode, queue,
packetize (packetizing the first FEC block and the first send call) and send
(the rest of the frame, with pacing). A point earlier than the one before it
counts as reached with it, so no stage is negative and the stages add up to
**present to sent**. Static repeats have no present and are not recorded.
The session's performance snapshot (`performance.stages`, shown in the
console as Present to sent) and the 5 s `stream timings` line give mean, p95
and p99 per stage over five seconds as `present_to_sent_ms` and
`stage_*_ms` (mean/p95/p99), beside `moonlight_host_ms`, the field sent to
Moonlight for the same pictures. `RUST_LOG=info,stages=trace` logs every
picture (target `stages`). The field sent to Moonlight is unchanged.

**Machine:** AMD Radeon RX 7900 XT (driver 32.0.32015.2008), Ryzen 7 5800X3D,
Windows 26200, one 5120x1440 monitor at 240 Hz. **Host:** the stage
instrumentation above (built before rebasing onto main `a5fb5155`, whose
newer commits change no host, capture or encoder code), release build, in a
copy of the 2.3.1 package, isolated profile on ports 48518-48544, AMF defaults,
`RUST_LOG=info,stages=trace`. **Physical display:** `rust/release/e2e.py`
streams 2560x1440 (the 5120x1440 desktop scaled into it) at 60, 120 and 240
fps and 50 Mbps. **Virtual display:** 1968x2184 at 120 fps and 80 Mbps on a
per-client virtual display in the **extended** layout, the host running as
SYSTEM from a one-shot scheduled task (the hardware-fps fixture's
`vd_e2e.py`); as SYSTEM, WGC always uses the helper. The layout was checked
unchanged after every run. **Capture:** WGC in the host process
(`wgc_user_helper = false`), the WGC helper (`wgc_user_helper = true`, as the
service runs) and Desktop Duplication (DDX). **Receiver:** the independent
moonlight-common-c fixture, decoding with D3D11VA on the same GPU, with
`BUTTERPOLLO_TEST_TIMING_CSV` per frame. `motion_probe` moves a 128-pixel
strip at twice the stream rate. **Load:** idle, or
`gpu_load SECONDS 1000 60 200` (60 fps, about 50% of the 3D engine).
20-second streams; the first 3 s and the last 1 s are dropped. Two runs per
case, the second pass in the reverse order; every cell is the mean of the two
runs' own statistics over 910-3,775 pictures each. Before each run the
installed host had no session and no app, no other test host ran, and the
3D engines were below 15%.

### Present to sent beside Moonlight's field

Each cell: present to sent mean / p95 / p99 in ms, with the mean of Moonlight's
field for the same pictures in parentheses. The receiver's per-frame host
latency, what Moonlight shows, matched the host's field to 0.08 ms in every
case.

| | 2560x1440, 60 fps | 2560x1440, 120 fps | 2560x1440, 240 fps | Virtual display 1968x2184, 120 fps |
| --- | ---: | ---: | ---: | ---: |
| HEVC DDX, idle | 5.0 / 8.4 / 12.2 (3.3) | 4.3 / 4.8 / 7.5 (3.3) | 4.3 / 6.2 / 7.8 (3.1) | 4.2 / 4.9 / 7.4 (3.4) |
| HEVC WGC, idle | 4.4 / 10.5 / 11.5 (3.2) | 4.4 / 7.2 / 9.4 (3.2) | 4.6 / 7.5 / 8.3 (3.2) | – |
| HEVC WGC helper, idle | 5.6 / 10.7 / 13.8 (3.2) | 4.4 / 6.4 / 8.4 (3.2) | 4.5 / 7.1 / 8.0 (3.2) | 3.9 / 4.5 / 4.9 (3.4) |
| AV1 DDX, idle | 4.3 / 6.0 / 11.8 (3.1) | 3.8 / 4.3 / 4.7 (3.0) | 4.1 / 5.5 / 7.7 (3.1) | 4.0 / 4.7 / 9.5 (3.2) |
| AV1 WGC, idle | 4.2 / 10.2 / 10.7 (2.9) | 4.2 / 6.7 / 7.2 (3.0) | 4.5 / 7.2 / 8.1 (3.0) | – |
| AV1 WGC helper, idle | 4.5 / 10.4 / 11.1 (3.0) | 3.8 / 6.0 / 8.4 (2.9) | 5.0 / 7.9 / 9.9 (3.2) | 3.6 / 4.7 / 5.1 (3.1) |
| HEVC DDX, GPU load | 7.4 / 12.2 / 14.3 (3.7) | 5.6 / 9.9 / 10.3 (3.5) | 5.2 / 7.7 / 8.6 (4.2) | 5.6 / 10.1 / 11.1 (3.6) |
| HEVC WGC, GPU load | 5.7 / 10.2 / 13.8 (3.2) | 9.0 / 15.1 / 18.7 (3.3) | 7.0 / 10.9 / 12.3 (3.3) | – |
| HEVC WGC helper, GPU load | 7.2 / 13.5 / 15.6 (3.2) | 7.5 / 11.8 / 14.6 (3.2) | 6.4 / 10.0 / 11.0 (3.2) | 6.8 / 10.8 / 12.1 (3.5) |
| AV1 DDX, GPU load | 8.2 / 13.8 / 14.1 (3.1) | 6.4 / 9.5 / 9.8 (3.9) | 6.7 / 9.5 / 20.6 (4.0) | 5.4 / 10.5 / 11.1 (3.2) |
| AV1 WGC, GPU load | 6.9 / 13.1 / 14.4 (3.0) | 6.1 / 10.2 / 11.1 (2.9) | 7.0 / 11.1 / 13.0 (3.4) | – |
| AV1 WGC helper, GPU load | 4.9 / 8.4 / 13.8 (3.0) | 6.2 / 10.3 / 11.2 (3.0) | 7.9 / 11.7 / 16.3 (3.4) | 5.8 / 10.3 / 10.8 (3.2) |

At idle, present to sent was 0.5-2.4 ms above Moonlight's field, about 1 ms
at 120 fps on the monitor. Under the GPU load it was 1.0-5.7 ms above, while
Moonlight's field moved by at most 1.1 ms. Moonlight's figure therefore says
little about how old a picture is when it leaves the host.

### Where the time goes

Stage means in ms. Hold is the WGC path's time between taking the picture and
publishing it; queue is the handoff to the sender.

| Case | fps received | Capture | Hold | Claim | Convert | Encode | Queue | Packetize | Send | Present to sent | Moonlight field | Moonlight shows |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2560x1440 60, HEVC, DDX, idle | 60.0 | 0.89 | 0.00 | 0.55 | 0.32 | 2.99 | 0.00 | 0.21 | 0.00 | 4.96 | 3.28 | 3.27 |
| 2560x1440 60, HEVC, WGC, idle | 60.0 | 0.27 | 0.06 | 0.62 | 0.34 | 2.92 | 0.01 | 0.21 | 0.00 | 4.43 | 3.24 | 3.24 |
| 2560x1440 60, HEVC, WGC helper, idle | 60.0 | 0.43 | 0.00 | 1.71 | 0.34 | 2.88 | 0.01 | 0.20 | 0.00 | 5.57 | 3.19 | 3.19 |
| 2560x1440 60, AV1, DDX, idle | 60.0 | 0.64 | 0.00 | 0.43 | 0.30 | 2.78 | 0.00 | 0.10 | 0.00 | 4.26 | 3.06 | 3.06 |
| 2560x1440 60, AV1, WGC, idle | 60.0 | 0.31 | 0.06 | 0.78 | 0.32 | 2.64 | 0.01 | 0.11 | 0.00 | 4.21 | 2.94 | 2.95 |
| 2560x1440 60, AV1, WGC helper, idle | 60.0 | 0.44 | 0.00 | 0.92 | 0.33 | 2.65 | 0.01 | 0.11 | 0.00 | 4.46 | 2.96 | 2.96 |
| 2560x1440 60, HEVC, DDX, GPU load | 60.0 | 0.76 | 0.00 | 2.69 | 0.37 | 3.37 | 0.00 | 0.23 | 0.01 | 7.43 | 3.71 | 3.70 |
| 2560x1440 60, HEVC, WGC, GPU load | 60.0 | 0.85 | 0.06 | 1.40 | 0.33 | 2.89 | 0.01 | 0.20 | 0.01 | 5.74 | 3.19 | 3.19 |
| 2560x1440 60, HEVC, WGC helper, GPU load | 60.1 | 1.63 | 0.00 | 2.14 | 0.35 | 2.90 | 0.01 | 0.21 | 0.00 | 7.24 | 3.22 | 3.22 |
| 2560x1440 60, AV1, DDX, GPU load | 60.0 | 0.55 | 0.00 | 4.48 | 0.35 | 2.75 | 0.00 | 0.10 | 0.00 | 8.23 | 3.08 | 3.08 |
| 2560x1440 60, AV1, WGC, GPU load | 60.1 | 0.91 | 0.06 | 2.78 | 0.33 | 2.65 | 0.01 | 0.11 | 0.00 | 6.85 | 2.96 | 2.96 |
| 2560x1440 60, AV1, WGC helper, GPU load | 60.1 | 0.74 | 0.00 | 1.10 | 0.35 | 2.63 | 0.01 | 0.11 | 0.00 | 4.93 | 2.96 | 2.96 |
| 2560x1440 120, HEVC, DDX, idle | 120.0 | 0.67 | 0.00 | 0.07 | 0.34 | 2.98 | 0.00 | 0.20 | 0.00 | 4.26 | 3.29 | 3.29 |
| 2560x1440 120, HEVC, WGC, idle | 120.0 | 0.58 | 0.06 | 0.30 | 0.36 | 2.88 | 0.01 | 0.22 | 0.00 | 4.41 | 3.21 | 3.22 |
| 2560x1440 120, HEVC, WGC helper, idle | 120.0 | 0.66 | 0.00 | 0.32 | 0.35 | 2.88 | 0.01 | 0.22 | 0.00 | 4.43 | 3.20 | 3.20 |
| 2560x1440 120, AV1, DDX, idle | 120.0 | 0.63 | 0.00 | 0.02 | 0.26 | 2.78 | 0.00 | 0.09 | 0.00 | 3.79 | 3.03 | 3.03 |
| 2560x1440 120, AV1, WGC, idle | 120.0 | 0.46 | 0.06 | 0.53 | 0.30 | 2.76 | 0.01 | 0.11 | 0.00 | 4.22 | 3.03 | 3.04 |
| 2560x1440 120, AV1, WGC helper, idle | 120.0 | 0.49 | 0.00 | 0.26 | 0.29 | 2.66 | 0.01 | 0.11 | 0.00 | 3.81 | 2.93 | 2.93 |
| 2560x1440 120, HEVC, DDX, GPU load | 120.0 | 0.61 | 0.00 | 1.20 | 0.31 | 3.23 | 0.00 | 0.22 | 0.00 | 5.57 | 3.51 | 3.50 |
| 2560x1440 120, HEVC, WGC, GPU load | 109.7 | 2.01 | 0.08 | 2.53 | 0.38 | 2.94 | 0.01 | 0.19 | 0.85 | 8.99 | 3.29 | 3.29 |
| 2560x1440 120, HEVC, WGC helper, GPU load | 120.0 | 1.50 | 0.00 | 2.47 | 0.35 | 2.89 | 0.01 | 0.24 | 0.00 | 7.46 | 3.21 | 3.22 |
| 2560x1440 120, AV1, DDX, GPU load | 120.0 | 0.59 | 0.00 | 1.78 | 0.27 | 3.63 | 0.00 | 0.12 | 0.00 | 6.38 | 3.88 | 3.82 |
| 2560x1440 120, AV1, WGC, GPU load | 120.1 | 0.74 | 0.06 | 2.25 | 0.29 | 2.66 | 0.01 | 0.12 | 0.00 | 6.13 | 2.93 | 2.94 |
| 2560x1440 120, AV1, WGC helper, GPU load | 120.1 | 0.96 | 0.00 | 2.10 | 0.29 | 2.69 | 0.01 | 0.12 | 0.00 | 6.17 | 2.96 | 2.96 |
| 2560x1440 240, HEVC, DDX, idle | 240.7 | 0.62 | 0.00 | 0.35 | 0.27 | 2.89 | 0.00 | 0.16 | 0.00 | 4.29 | 3.14 | 3.14 |
| 2560x1440 240, HEVC, WGC, idle | 239.8 | 0.42 | 0.06 | 0.72 | 0.27 | 2.94 | 0.00 | 0.15 | 0.00 | 4.57 | 3.19 | 3.19 |
| 2560x1440 240, HEVC, WGC helper, idle | 239.7 | 0.36 | 0.00 | 0.74 | 0.27 | 2.97 | 0.00 | 0.14 | 0.00 | 4.49 | 3.20 | 3.19 |
| 2560x1440 240, AV1, DDX, idle | 240.3 | 0.66 | 0.00 | 0.26 | 0.30 | 2.78 | 0.00 | 0.13 | 0.00 | 4.13 | 3.06 | 3.05 |
| 2560x1440 240, AV1, WGC, idle | 238.9 | 0.49 | 0.06 | 0.75 | 0.28 | 2.76 | 0.00 | 0.12 | 0.00 | 4.47 | 3.03 | 3.02 |
| 2560x1440 240, AV1, WGC helper, idle | 238.0 | 0.65 | 0.00 | 0.96 | 0.32 | 2.90 | 0.00 | 0.13 | 0.00 | 4.96 | 3.17 | 3.18 |
| 2560x1440 240, HEVC, DDX, GPU load | 240.1 | 0.55 | 0.00 | 0.24 | 0.25 | 3.97 | 0.00 | 0.16 | 0.00 | 5.17 | 4.20 | 4.13 |
| 2560x1440 240, HEVC, WGC, GPU load | 231.7 | 1.27 | 0.07 | 1.45 | 0.30 | 3.00 | 0.00 | 0.13 | 0.75 | 6.97 | 3.27 | 3.28 |
| 2560x1440 240, HEVC, WGC helper, GPU load | 240.5 | 1.34 | 0.00 | 1.63 | 0.27 | 3.01 | 0.00 | 0.15 | 0.00 | 6.42 | 3.23 | 3.24 |
| 2560x1440 240, AV1, DDX, GPU load | 193.6 | 0.64 | 0.00 | 0.78 | 0.29 | 3.75 | 0.00 | 0.08 | 1.16 | 6.70 | 4.01 | 3.94 |
| 2560x1440 240, AV1, WGC, GPU load | 210.5 | 1.26 | 0.07 | 1.29 | 0.31 | 3.06 | 0.00 | 0.07 | 0.95 | 7.02 | 3.35 | 3.27 |
| 2560x1440 240, AV1, WGC helper, GPU load | 200.1 | 1.82 | 0.00 | 1.64 | 0.32 | 3.17 | 0.00 | 0.08 | 0.91 | 7.94 | 3.43 | 3.42 |
| VD 1968x2184 120, HEVC, DDX, idle | 119.9 | 0.54 | 0.00 | 0.11 | 0.31 | 3.12 | 0.00 | 0.10 | 0.00 | 4.17 | 3.40 | 3.40 |
| VD 1968x2184 120, HEVC, WGC helper, idle | 120.0 | 0.06 | 0.00 | 0.32 | 0.30 | 3.12 | 0.01 | 0.10 | 0.00 | 3.92 | 3.40 | 3.40 |
| VD 1968x2184 120, AV1, DDX, idle | 119.6 | 0.60 | 0.00 | 0.10 | 0.36 | 2.84 | 0.00 | 0.08 | 0.00 | 3.98 | 3.18 | 3.18 |
| VD 1968x2184 120, AV1, WGC helper, idle | 120.0 | 0.07 | 0.00 | 0.32 | 0.33 | 2.81 | 0.01 | 0.07 | 0.00 | 3.61 | 3.12 | 3.12 |
| VD 1968x2184 120, HEVC, DDX, GPU load | 119.8 | 0.57 | 0.00 | 1.33 | 0.40 | 3.22 | 0.00 | 0.10 | 0.00 | 5.62 | 3.60 | 3.61 |
| VD 1968x2184 120, HEVC, WGC helper, GPU load | 115.1 | 0.90 | 0.00 | 2.27 | 0.38 | 3.13 | 0.01 | 0.10 | 0.02 | 6.81 | 3.49 | 3.49 |
| VD 1968x2184 120, AV1, DDX, GPU load | 119.9 | 0.55 | 0.00 | 1.47 | 0.37 | 2.88 | 0.00 | 0.08 | 0.00 | 5.35 | 3.23 | 3.24 |
| VD 1968x2184 120, AV1, WGC helper, GPU load | 119.9 | 0.75 | 0.00 | 1.80 | 0.34 | 2.84 | 0.01 | 0.07 | 0.00 | 5.81 | 3.16 | 3.16 |

Stage p99 in ms:

| Case | Capture | Hold | Claim | Convert | Encode | Queue | Packetize | Send |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2560x1440 60, HEVC, DDX, idle | 3.07 | 0.00 | 8.12 | 0.60 | 3.50 | 0.00 | 0.38 | 0.11 |
| 2560x1440 60, HEVC, WGC, idle | 2.02 | 0.15 | 6.91 | 0.63 | 3.46 | 0.01 | 0.40 | 0.11 |
| 2560x1440 60, HEVC, WGC helper, idle | 2.31 | 0.00 | 9.73 | 0.66 | 3.45 | 0.01 | 0.37 | 0.00 |
| 2560x1440 60, AV1, DDX, idle | 2.57 | 0.00 | 8.06 | 0.55 | 3.31 | 0.00 | 0.24 | 0.00 |
| 2560x1440 60, AV1, WGC, idle | 1.91 | 0.18 | 6.91 | 0.62 | 3.31 | 0.01 | 0.22 | 0.00 |
| 2560x1440 60, AV1, WGC helper, idle | 2.19 | 0.00 | 7.10 | 0.58 | 3.23 | 0.01 | 0.22 | 0.00 |
| 2560x1440 60, HEVC, DDX, GPU load | 4.60 | 0.00 | 10.11 | 0.81 | 6.48 | 0.00 | 0.40 | 0.13 |
| 2560x1440 60, HEVC, WGC, GPU load | 3.46 | 0.16 | 9.77 | 0.65 | 3.49 | 0.01 | 0.37 | 0.11 |
| 2560x1440 60, HEVC, WGC helper, GPU load | 6.48 | 0.00 | 10.90 | 0.69 | 3.61 | 0.01 | 0.38 | 0.21 |
| 2560x1440 60, AV1, DDX, GPU load | 2.43 | 0.00 | 10.25 | 0.75 | 3.37 | 0.00 | 0.23 | 0.00 |
| 2560x1440 60, AV1, WGC, GPU load | 4.47 | 0.22 | 10.31 | 0.68 | 3.17 | 0.01 | 0.24 | 0.00 |
| 2560x1440 60, AV1, WGC helper, GPU load | 4.70 | 0.00 | 8.12 | 0.72 | 3.18 | 0.01 | 0.23 | 0.00 |
| 2560x1440 120, HEVC, DDX, idle | 1.47 | 0.00 | 1.21 | 0.67 | 3.68 | 0.00 | 0.32 | 0.00 |
| 2560x1440 120, HEVC, WGC, idle | 2.81 | 0.22 | 4.64 | 0.76 | 3.44 | 0.01 | 0.40 | 0.00 |
| 2560x1440 120, HEVC, WGC helper, idle | 2.92 | 0.00 | 2.67 | 0.74 | 3.52 | 0.01 | 0.40 | 0.00 |
| 2560x1440 120, AV1, DDX, idle | 1.44 | 0.00 | 0.03 | 0.51 | 3.40 | 0.00 | 0.17 | 0.00 |
| 2560x1440 120, AV1, WGC, idle | 1.54 | 0.18 | 3.35 | 0.55 | 3.30 | 0.01 | 0.21 | 0.00 |
| 2560x1440 120, AV1, WGC helper, idle | 2.90 | 0.00 | 4.55 | 0.61 | 3.26 | 0.01 | 0.23 | 0.00 |
| 2560x1440 120, HEVC, DDX, GPU load | 1.60 | 0.00 | 6.04 | 0.66 | 5.45 | 0.00 | 0.37 | 0.00 |
| 2560x1440 120, HEVC, WGC, GPU load | 8.91 | 0.31 | 8.26 | 0.83 | 3.58 | 0.01 | 0.38 | 2.13 |
| 2560x1440 120, HEVC, WGC helper, GPU load | 7.12 | 0.00 | 7.42 | 0.75 | 3.50 | 0.01 | 0.41 | 0.00 |
| 2560x1440 120, AV1, DDX, GPU load | 1.60 | 0.00 | 6.15 | 0.53 | 5.97 | 0.00 | 0.21 | 0.00 |
| 2560x1440 120, AV1, WGC, GPU load | 5.19 | 0.22 | 6.82 | 0.66 | 3.26 | 0.01 | 0.23 | 0.00 |
| 2560x1440 120, AV1, WGC helper, GPU load | 5.97 | 0.00 | 6.67 | 0.66 | 3.45 | 0.01 | 0.22 | 0.00 |
| 2560x1440 240, HEVC, DDX, idle | 1.36 | 0.00 | 3.46 | 0.55 | 3.72 | 0.00 | 0.27 | 0.00 |
| 2560x1440 240, HEVC, WGC, idle | 2.41 | 0.21 | 3.96 | 0.62 | 4.02 | 0.01 | 0.27 | 0.00 |
| 2560x1440 240, HEVC, WGC helper, idle | 2.43 | 0.00 | 3.75 | 0.61 | 4.25 | 0.01 | 0.26 | 0.00 |
| 2560x1440 240, AV1, DDX, idle | 1.31 | 0.00 | 3.30 | 0.66 | 4.13 | 0.00 | 0.19 | 0.00 |
| 2560x1440 240, AV1, WGC, idle | 2.44 | 0.20 | 3.85 | 0.61 | 4.14 | 0.01 | 0.19 | 0.00 |
| 2560x1440 240, AV1, WGC helper, idle | 3.92 | 0.00 | 3.69 | 0.72 | 4.58 | 0.01 | 0.19 | 0.00 |
| 2560x1440 240, HEVC, DDX, GPU load | 1.20 | 0.00 | 3.80 | 0.51 | 7.20 | 0.00 | 0.28 | 0.00 |
| 2560x1440 240, HEVC, WGC, GPU load | 6.50 | 0.24 | 5.15 | 0.68 | 4.15 | 0.01 | 0.34 | 1.81 |
| 2560x1440 240, HEVC, WGC helper, GPU load | 6.16 | 0.00 | 5.16 | 0.63 | 4.31 | 0.01 | 0.29 | 0.00 |
| 2560x1440 240, AV1, DDX, GPU load | 1.44 | 0.00 | 4.09 | 0.62 | 8.28 | 0.00 | 0.30 | 2.97 |
| 2560x1440 240, AV1, WGC, GPU load | 7.72 | 0.30 | 5.03 | 0.75 | 4.35 | 0.01 | 0.21 | 2.37 |
| 2560x1440 240, AV1, WGC helper, GPU load | 6.79 | 0.00 | 4.84 | 0.71 | 4.64 | 0.01 | 0.31 | 2.57 |
| VD 1968x2184 120, HEVC, DDX, idle | 1.23 | 0.00 | 2.71 | 0.59 | 3.63 | 0.00 | 0.18 | 0.00 |
| VD 1968x2184 120, HEVC, WGC helper, idle | 0.14 | 0.00 | 0.89 | 0.60 | 3.62 | 0.01 | 0.18 | 0.00 |
| VD 1968x2184 120, AV1, DDX, idle | 1.46 | 0.00 | 5.38 | 0.65 | 3.34 | 0.00 | 0.12 | 0.00 |
| VD 1968x2184 120, AV1, WGC helper, idle | 0.46 | 0.00 | 1.70 | 0.67 | 3.29 | 0.01 | 0.11 | 0.00 |
| VD 1968x2184 120, HEVC, DDX, GPU load | 1.42 | 0.00 | 6.89 | 0.75 | 7.21 | 0.00 | 0.17 | 0.00 |
| VD 1968x2184 120, HEVC, WGC helper, GPU load | 5.08 | 0.00 | 7.22 | 0.82 | 3.74 | 0.01 | 0.17 | 0.00 |
| VD 1968x2184 120, AV1, DDX, GPU load | 1.27 | 0.00 | 7.09 | 0.79 | 3.60 | 0.00 | 0.13 | 0.00 |
| VD 1968x2184 120, AV1, WGC helper, GPU load | 3.91 | 0.00 | 7.17 | 0.73 | 3.45 | 0.01 | 0.12 | 0.00 |

- **Encoding is the largest part.** AMF took 2.6-3.1 ms from accepting a
  picture to returning its frame at idle, conversion 0.26-0.36 ms before it.
  Moonlight's field is essentially conversion plus encoding.
- **Capture with Desktop Duplication is about 0.6 ms** (0.54-0.89 ms) at
  every rate, idle or loaded, on the monitor and on the virtual display (p99
  1.2-1.6 ms at 120 and 240 fps; up to 4.6 ms at 60 fps).
- **The wait for the claim** is pacing: a published picture waits for its
  slot. At idle it was 0.02-1.7 ms, least with DDX at 120 fps; under load
  1.1-4.5 ms at 60 and 120 fps and 0.2-2.3 ms at 240 fps and on the virtual
  display.
- **Under load, DDX's delay moves into encoding.** Desktop Duplication hands
  over a picture before DWM's GPU work on it has finished
  ([October 12 diagnosis](PERFORMANCE_WORK.md#av1-host-processing-p95-diagnosis-october-12));
  behind the game that work finishes later, so AMF's encode stage grew by
  0.25-1.1 ms at 120 and 240 fps (p99 up to 8.3 ms), while DDX's capture
  stage stayed at 0.6 ms. WGC's encode grew by at most 0.3 ms; its pictures
  arrived later instead.
- **Packetization took 0.07-0.24 ms**; the queue to the sender thread was
  unused (these loopback streams send inline). **Sending** was near zero on
  loopback, except in runs where the loaded receiver lost frames and asked for
  4-32 keyframes. The host took those requests for keyframes lost on the
  way ("keyframes are not arriving"), paced the rest of the stream at twice
  the bitrate (`keyframe_loss_pacing`), and sending a frame took 0.75-1.2 ms
  on average over the two runs, p99 up to 3 ms. On a real network the paced
  send adds the frame's size over the pacing rate to every frame.

### Capture: how late is the picture in hand

Capture stage, present stamp to frame in hand, mean / p99 ms:

| | 2560x1440, 60 fps | 2560x1440, 120 fps | 2560x1440, 240 fps | Virtual display 1968x2184, 120 fps |
| --- | ---: | ---: | ---: | ---: |
| HEVC DDX, idle | 0.89 / 3.07 | 0.67 / 1.47 | 0.62 / 1.36 | 0.54 / 1.23 |
| HEVC WGC, idle | 0.27 / 2.02 | 0.58 / 2.81 | 0.42 / 2.41 | – |
| HEVC WGC helper, idle | 0.43 / 2.31 | 0.66 / 2.92 | 0.36 / 2.43 | 0.06 / 0.14 |
| AV1 DDX, idle | 0.64 / 2.57 | 0.63 / 1.44 | 0.66 / 1.31 | 0.60 / 1.46 |
| AV1 WGC, idle | 0.31 / 1.91 | 0.46 / 1.54 | 0.49 / 2.44 | – |
| AV1 WGC helper, idle | 0.44 / 2.19 | 0.49 / 2.90 | 0.65 / 3.92 | 0.07 / 0.46 |
| HEVC DDX, GPU load | 0.76 / 4.60 | 0.61 / 1.60 | 0.55 / 1.20 | 0.57 / 1.42 |
| HEVC WGC, GPU load | 0.85 / 3.46 | 2.01 / 8.91 | 1.27 / 6.50 | – |
| HEVC WGC helper, GPU load | 1.63 / 6.48 | 1.50 / 7.12 | 1.34 / 6.16 | 0.90 / 5.08 |
| AV1 DDX, GPU load | 0.55 / 2.43 | 0.59 / 1.60 | 0.64 / 1.44 | 0.55 / 1.27 |
| AV1 WGC, GPU load | 0.91 / 4.47 | 0.74 / 5.19 | 1.26 / 7.72 | – |
| AV1 WGC helper, GPU load | 0.74 / 4.70 | 0.96 / 5.97 | 1.82 / 6.79 | 0.75 / 3.91 |

WGC's `SystemRelativeTime` is not a present time: for 47-99% of idle frames it
was later than the frame's arrival at the host (24-95% under load), and the
host then uses the arrival. WGC's capture stage therefore leaves out how long
WGC took to deliver. To estimate that part, the receiver's render-to-received
time (the probe's QPC before `Present` to the frame fully received on the same
clock) was compared with present to sent: their difference is the time from
the probe's `Present` to the host's stamp, plus loopback. With DDX the stamp is
the compositor's present; the difference between WGC's and DDX's, using
medians, is the delivery WGC hides:

| | 2560x1440, 60 fps | 2560x1440, 120 fps | 2560x1440, 240 fps | Virtual display 1968x2184, 120 fps |
| --- | ---: | ---: | ---: | ---: |
| HEVC WGC, idle | 2.5 | 2.1 | 2.0 | – |
| HEVC WGC helper, idle | 3.5 | 2.3 | 2.1 | 0.9 |
| AV1 WGC, idle | 1.5 | 2.2 | 2.1 | – |
| AV1 WGC helper, idle | 2.8 | 1.9 | 2.7 | 0.8 |
| HEVC WGC, GPU load | 2.5 | 3.7 | 2.1 | – |
| HEVC WGC helper, GPU load | 3.8 | 4.3 | 2.6 | -0.7 |
| AV1 WGC, GPU load | 1.7 | 1.5 | 3.3 | – |
| AV1 WGC helper, GPU load | 1.9 | 3.4 | 3.4 | -1.8 |

At idle it is about 2 ms on the 240 Hz monitor (1.9-2.7 ms at 120 and 240
fps; 1.5-3.5 ms at 60 fps, where the estimate is noisier) and 0.8-0.9 ms on
the virtual display, close to the helper's 0.7-1.0 ms delivery gap in the
[October 7 probe](#october-7-where-wgc-loses-a-millisecond). Under load the
monitor's estimates are 1.5-4.3 ms; on the virtual display the estimate came
out negative, with DDX's loaded present to sent there skewed (medians 3.9-4.4
ms, means 5.1-5.8 ms), and is not usable. So WGC's real capture stage is about 2.3-3.3 ms idle on the monitor at
120 and 240 fps and about 0.9 ms on the virtual display, against DDX's
0.5-0.7 ms.

### Comparable numbers

[punktfunk](https://git.unom.io/unom/punktfunk) 0.44's native `host_us`
runs from the compositor's present stamp its virtual display driver passes
with each frame to that frame's last paced packet
(`crates/host/punktfunk-host/src/native/stream/send.rs`). Rubylight's present
to sent with DDX covers that span, from the compositor's present stamp
(`LastPresentTime`) to the last packet: 3.8-5.0 ms idle and 5.2-8.2 ms under
this load on the monitor, 4.0-4.2 ms idle and 5.4-5.6 ms loaded on the
virtual display. With WGC, add the hidden delivery above: roughly 5.7-7.7 ms
idle on the monitor at 120 and 240 fps and 4.4-4.8 ms on the virtual display.
punktfunk's Moonlight field (`gamestream/stream.rs`, `cap_at` to the
packetizer thread) starts at its
encode loop's tick and, with its encoding driver, leaves out the encode; it
cannot be compared with Rubylight's Moonlight field or with present to sent.
No punktfunk host ran on this machine; these are Rubylight's numbers for the
span punktfunk reports.

### Is a frame-push display driver worth it

A driver that hands the host each composed frame at the compositor's present
can only remove the capture stage, and only down to its own handoff:

- **Against DDX, at most about 0.6 ms** a frame, idle or loaded, at every
  rate. DDX's capture p99 was 1.2-1.6 ms at 120 and 240 fps.
- **Against WGC, about 2.3-3.3 ms idle on the monitor** (capture plus hidden
  delivery), about 0.9 ms on the virtual display, and an estimated 2-6 ms
  under load on the monitor.
- Under load the larger costs are elsewhere: the wait for the claim
  (1.1-4.5 ms at 60 and 120 fps) and, with DDX, encoding behind DWM's
  unfinished GPU work (up to 1.1 ms more). A driver that only delivers frames
  earlier does not shorten the claim wait; whether a pushed frame avoids
  DDX's wait for DWM's work is not measured here.

Against the WGC helper the service uses, a push driver could save about
2.3-3.3 ms a frame idle on a monitor and about 1 ms on a virtual display.
Against DDX it could save about 0.6 ms.

### Limits

One machine, one GPU, one driver. The receiver decodes on the same GPU; under
load it lost frames with AV1 at 240 fps (194-211 fps received) and with
in-process WGC HEVC at 120 and 240 fps (110 and 232 fps). At 240 fps the probe
presents about 233 times a second, so the receiver's motion check fails there
although it received 238-241 fps idle. Loopback hides network pacing.
The hidden-delivery estimate subtracts distributions, not matched frames. The
virtual display's AV1 WGC-helper load case of the second pass failed to launch
(the client was refused before the stream started) and was run again.

Reproduce with `run_matrix.py`, `vd_cases.ps1`, `collect_vd.py`, `stages.py`
and `tables.py` in `C:\src\butterpollo-release\stage-latency` (the case list is
`CODEC-FPS-CAPTURE-LOAD`), with the host log at `info,stages=trace` and
`BUTTERPOLLO_TEST_TIMING_CSV` set for the receiver. Raw runs: `runs1`,
`runs2`, `vdruns1`, `vdruns2`; summaries in `matrix1.jsonl`, `matrix2.jsonl`,
`vd1.jsonl` and `vd2.jsonl`.

## October 15: frame push from Rubylight's display driver

`capture = idd-push` takes each frame from Rubylight's own IddCx driver
(`rust/drivers/rubylight-vdisplay`) through a shared texture ring instead of
asking Windows for it. The estimate above put the most it can save at the
capture stage: about 0.9 ms on a virtual display.

**Machine and host** as in the section above; host from `claude/idd-push`
rebased on main (release build, isolated profile), driver 0.1.0.6912
test-signed on the same PC with Memory Integrity on. `vd_e2e.py` streams a
per-client Rubylight virtual display (extended layout, run as SYSTEM) at
2560x1440, 80 Mbps, 20 s per case, with `virtual_display_driver = rubylight`
and `capture = idd-push` or `wgc`; the receiver decodes on the same GPU.
Two passes, the second with stage tracing. **Render to received** is the
receiver's clock from the probe's present on the virtual display to the
frame fully received, so it includes what happens before any host stamp.

| Rate | Render to received, idd-push faster by (mean of 8 pairs) | Pairs idd-push won | Range |
| --- | ---: | ---: | ---: |
| 60 Hz | -0.84 ms | 2 of 8 | -2.61 to +2.58 ms |
| 120 Hz | +0.87 ms | 6 of 8 | -1.80 to +2.77 ms |
| 240 Hz | +0.92 ms | 8 of 8 | +0.58 to +1.41 ms |

At 240 Hz (HEVC, AV1, each SDR and HDR, both passes) idd-push received
frames in 8.7-9.9 ms against 9.4-10.6 ms with WGC, and its p95 was the same
or lower in 7 of 8 pairs (10.8-15.1 against 10.8-15.1 ms; HDR 11.3-13.1
against 12.6-14.1 ms). The host's own present to sent does not show the gain:
WGC's capture stage on a virtual display is only 0.05 ms (its frame time is
close to its arrival), and idd-push's present to sent was 0.1 ms longer at
240 Hz (2.90-3.22 against 2.77-3.10 ms), probably because its present stamp is the
driver's, taken earlier. The saving is in Windows' delivery to WGC, before any
stamp WGC gives the host. At 60 Hz the 16.7 ms frame period dominates and
idd-push's claim waited longer (0.4-2.4 against 0.7-1.0 ms mean); whether that
is the pacing phase or the push is not settled. Delivered frame rates at
240 Hz were mixed (194-229 against 203-235 fps); both captures failed the
receiver's gap check in most cases on this loaded virtual display, at the same
rate.

So idd-push reaches the 0.9 ms the capture stage allowed on a virtual display
at 240 Hz, and about that at 120 Hz with more spread. It stays opt-in
(`virtual_display_driver = rubylight`); the driver is not in the installer.

Reproduce with `cases.ps1`, `run.ps1` and `collect.py` in
`C:\src\butterpollo-release\idd-push` (labels `CODEC[-hdr]-FPS-CAPTURE`).
Raw runs: `runs1`, `runs2`; summaries `matrix1.jsonl` and `matrix2.jsonl`.

## October 15: display driver and frame push refactor

The display driver and the host's `idd-push` path were reworked for latency.
What changed, and what each stage costs now:

- **The claim wait at 60 fps was the pacer, not the driver.** The 2x virtual
  display is a faster source, whose credit refilled at exactly the stream
  rate, so a deficit from the slow first encode was never repaid: every claim
  waited the same 1-3 ms beside a frame already in hand (480 of 900 frames of
  the earlier HEVC 60 fps run sat at 2-3 ms). A frame that waits only for
  credit now borrows the shortfall from a bounded grace (a quarter frame at
  most, refilled at a tenth of a frame a second). The claim stage at 60 fps
  fell from 0.37-2.39 ms to 0.07-0.28 ms mean, and at 120 fps from 0.59-1.37
  to 0.06-0.07 ms.
- **No host copy.** On AMD the compute conversion reads the ring texture the
  driver wrote, holding its keyed mutex until the conversion is done (as the
  WGC helper's zero-copy path does). Convert fell from 0.20-0.26 to 0.16-0.21
  ms mean. Four textures were enough: the driver dropped and overwrote no
  frame in any run.
- **An honest capture stage.** IddCx's `PresentDisplayQPCTime` is the
  scan-out, 6.8 ms after the driver acquired the frame on a 120 Hz display,
  so the capture stage used to read 0.00. The present stamp is now the
  driver's acquire: capture (acquire to the host taking the texture) is
  0.13-0.24 ms.
- **The driver costs about nothing.** From the swap chain's acquire to the
  publish took 19 us on average (p99 55 us); from the publish to the host
  taking the texture, 132 us (p99 221 us), the capture thread's wake.
- **What remains is encode.** AMF's encode stage on the driver's frames is
  2.8-3.7 ms against 2.6-2.8 ms with WGC on the same display. The driver gets
  DWM's surface before DWM's GPU work on it has finished, and the copy and
  conversion queue behind it, as with Desktop Duplication.

**Machine and method** as in the section above: RX 7900 XT, AMF, Ryzen 7
5800X3D, Windows 26200, Memory Integrity on; driver 0.1.0.7001 signed on the
PC; release host from this work; 2560x1440 per-client virtual display,
extended layout, host as SYSTEM, 20 s per case, receiver decoding on the same
GPU. One full pass of HEVC and AV1, SDR and HDR, 60/120/240 fps (30 cases
with exact refresh, 28 passed the receiver's checks), then the 240 fps half
of a reverse-order pass. The other half of that pass was discarded: from
15:32 the desktop was in use and DWM composed only half the virtual
display's frames (62 instead of 120 a second) for both captures.

Render to received, mean / p95 ms, first pass:

| | 60 fps | 120 fps | 240 fps |
| --- | ---: | ---: | ---: |
| HEVC idd-push | 8.44 / 8.92 | 8.37 / 8.86 | 8.38 / 8.95 |
| AV1 idd-push | 6.40 / 6.90 | 8.18 / 8.73 | 8.15 / 8.68 |
| HEVC HDR idd-push | 6.71 / 7.27 | 8.25 / 8.77 | 8.30 / 8.81 |
| AV1 HDR idd-push | 8.78 / 9.39 | 8.16 / 8.73 | 7.83 / 8.34 |

The second pass at 240 fps agreed within 0.3 ms (7.87-8.34 ms). p95 is now
within 0.7 ms of the mean in every case; before the refactor it was 2.5-9.6
ms above it. For reference, WGC on the same display was 0.6-1.0 ms slower at
120 and 240 fps and 1.1-2.8 ms faster at 60 fps, where the difference lies
before any host stamp and is not explained yet.

**Exact refresh does not pay.** A display at the stream rate
(`frame_limiter_auto_virtual_framegen = disabled`) instead of twice it was
0.12-0.14 ms faster at 240 fps, 0.7 ms slower at 120 fps and 2.2 ms slower
at 60 fps (first pass), so the 2x default stays.

Reproduce with `cases.ps1` (`-LabelList`, labels `CODEC[-hdr]-FPS-CAPTURE[-1x]`),
`run.ps1`, `collect.py` and `compare.py` in `C:\src\butterpollo-release\idd-push`;
`idd_push=trace` in `log-extra.txt` logs the driver's per-frame times. Raw runs
`runs3`, `runs4`, `runs-diag-*`; summaries `matrix3.jsonl`, `matrix4.jsonl`.

### Later that day: driver only, and what else was tried

The host now creates every virtual display with this driver (libvirtualdisplay
and its lease protocol are gone), so `capture = auto` always takes the frame
push on them. Three host builds on the same driver, idd-push only, HEVC and
AV1 HDR at 60/120/240 fps, two rounds in alternating order (`runs-final`,
`matrix-final.jsonl`): the build before the refactor, the refactor, and the
refactor with the capture thread kept at its MMCSS boost. The main monitor had
been switched from 5120x1440 to 2560x1440 since the morning, and render to
received was about 13 ms for every build (8 ms in the morning), so only the
differences within the run count; they are about 0.3 ms of noise.

| Render to received, mean ms | 60 fps | 120 fps | 240 fps |
| --- | ---: | ---: | ---: |
| HEVC, before the refactor | 14.18 | 13.22 | 13.31 |
| HEVC, refactor | 13.48 | 12.98 | 12.90 |
| AV1 HDR, before the refactor | 13.45 | 12.96 | 13.15 |
| AV1 HDR, refactor | 14.00 | 13.19 | 12.89 |

The refactor is 0.3-0.45 ms faster at 240 fps and for HEVC at 60 and 120 fps,
and 0.2-0.55 ms slower for AV1 HDR at 60 and 120 fps. Three more ideas did not
pay and were dropped:

- **The capture thread at its MMCSS boost** (as the input thread is): the
  handoff from the driver's publish to the host taking the frame stayed at
  130-155 us (119-139 us without it), and at 60 fps the claim wait and p95
  grew.
- **Waking the host before the driver's flush**: handoff 132-179 us against
  127-141 us.
- **Converting in the driver** into the encoder's NV12/P010 (bit-exact with
  the host's compute conversion): AMF blocks 1-3 ms in `SubmitInput` on
  textures opened from the ring's shared handle, and with a copy into the
  converter's own texture first, the conversion runs on the driver's graphics
  queue behind DWM instead of the host's compute queue, 0.3-1.3 ms later at
  60 and 120 fps. The ring stays a copy the host converts in place.
- **A faster virtual display** to shorten DWM's wait for its vblank before the
  driver acquires: four times the stream rate (`enabled`) and the fixed
  1000 Hz (`vrr`) against the 2x default, HEVC, one 8 s round each
  (`runs-refresh`, `cases-refresh.ps1`). 4x was within 0.15 ms of 2x
  (60 fps 13.03 against 13.15 ms render to received, 120 fps 12.71 against
  12.72); 1000 Hz was no faster at 120 fps and worse at 60 fps (14.28 ms, p95
  20.8 against 13.6 ms, the claim wait 0.65 against 0.08 ms). The 240 fps
  cases delivered 120 fps in this run and agree with the 120 fps ones. The 2x
  default stays.

A code review of the path from the driver to the socket found the encoder
(ultra-low-latency usage, no pre-analysis or B-frames, output queried every
100 us), the conversion (a GPU fence, 0.18 ms) and the driver loop (event
waits, one copy) already lean, and four candidates. One A/B round, HEVC unless
named (`runs-ab`, `cases-ab.ps1`). Before it, setup installed package
2.3.1.6905 (main at 60d16e62) to `C:\Program Files\Rubylight`, replacing
the branch-built driver and service and removing libvirtualdisplay's HDR layer
and device, and the main monitor was switched back to its morning mode. Render
to received at 120 fps fell from about 12.7 to 8.7 ms (60 fps 13.2 to 11.1)
with the host and harness unchanged: the monitor mode, as the morning's 8 ms
suggests, sets the time before the driver's acquire. Compare within this
table only:

| Render to received, mean / p95 ms | 60 fps | 120 fps |
| --- | ---: | ---: |
| Extended layout (the harness default) | 11.09 / 11.43 | 8.68 / 9.04 |
| Exclusive layout (the shipped default) | 12.16 / 12.99 | 8.24 / 8.83 |
| Extended, virtual display primary | 13.06 / 18.17 | 8.17 / 8.92 |
| Pushed frames claimed half a period apart, 1 ms detection | 9.56 / 16.92 | 8.85 / 9.47 |
| AV1, 8-bit | | 8.57 / 8.97 |

- **Wired pacing at 950 Mbps on a gigabit host link** (from 800 Mbps) was
  shipped and reverted. The harness streams over loopback, so it was measured
  on the LAN instead: `pacing_link_probe` sends frame-sized bursts through the
  stream's packet pacer from this PC (2.5 GbE) to `pacing_receiver` on the
  gigabit NUC at 192.168.4.10, which records kernel arrival times; a keyframe
  eight times the size every five seconds, 10 s per case, two rounds in
  alternating order (`C:\src\butterpollo-release\pacing-nuc`):

  | Wire rate | Pacing | P-frame arrival span, mean ms | Packets lost |
  | --- | ---: | ---: | ---: |
  | 100 Mbps, 60 fps | 800 Mbps | 1.87 / 1.91 | 0 / 0 |
  | | 950 Mbps | 1.87 / 2.06 | 0 / 0 |
  | 100 Mbps, 120 fps | 800 Mbps | 0.96 / 0.95 | 0 / 0 |
  | | 950 Mbps | 0.95 / 0.99 | 0 / 359 |
  | 150 Mbps, 60 fps | 800 Mbps | 3.01 / 3.19 | 125 / 431 |
  | | 950 Mbps | 2.79 / 2.81 | 283 / 46 |

  The receiver's gigabit port, not the pacing, sets when a frame's last
  packet arrives at ordinary bitrates; keyframes arrived 2-3 ms sooner at
  60 fps and 150 Mbps frames 0.2-0.4 ms. Every loss was the receiver's
  416 KB socket buffer overflowing, and 950 Mbps lost 359 packets in a round
  where 800 Mbps lost none, so 800 Mbps stays.
- **Recovery frames get the configured FEC.** Adaptive FEC falls to 5% on a
  clean link, and a Wi-Fi dropout that loses whole frames leaves the client
  no partial frame to report, so the keyframe or invalidation frame that
  repairs it went out at 5%, on a link that had just failed. Keyframes and
  the frame after an invalidation now get `fec_percentage` (20% by default);
  other frames keep the adaptive percentage. A 60 s HEVC 120 fps stream with
  an 80 ms dropout at 45 s, after adaptive FEC had decayed (`runs-fec`),
  recovered alike with and without it: every frame decoded, picture age p95
  17.3 against 17.6 ms, longest gap 88 against 97 ms. Loopback loses nothing
  after the dropout, so the gain, keyframes surviving the loss that follows
  one, is not measured here.
- **The display layout** moves the time before the driver's acquire by
  0.4-0.5 ms at 120 fps and 1-2 ms the other way at 60 fps: no consistent
  gain, and the harness's extended layout stands in for the shipped default.
- **Claiming pushed frames half a period apart** (as VRR does) with a 1 ms wait
  for a predicted frame instead of a quarter period: the mean fell at 60 fps,
  but p95 rose 5.5 ms, picture age p95 from 13.5 to 19.1 ms and the claim wait
  from 0.07 to 2.12 ms; at 120 fps it was 0.2 ms slower and the p99 frame
  interval rose from 9.2 to 11.5 ms. Dropped (branch `tried/pushed-pacer`
  in the worktree only).
- **8-bit AV1** was 0.1 ms faster than HEVC end to end, not the 0.5 ms the
  encoder alone measured; nothing to change on the host.

## October 18 pace-based recovery policy

The default AMF frame cap now follows the current conventional-video packet pace rather than a fixed 2× frame budget. The pure calculation reserves two frame periods of transmission, accounts for configured recovery FEC and packet/header overhead (including encryption conservatively), and clamps the encoded-byte budget to 2–8 nominal frames. At the earlier fixture's 54,097 Kbps, 119.88 FPS, 20% FEC and 70.77 Mbps pace, this retains the 2× lower bound; at 800 Mbps it reaches 8×. These are calculated property targets, not measured decoder latency or picture quality. The cap remains soft and the lower bound cannot fit a severely undersized link into two periods.

Encoder admission previews the live route/adaptive/loss policy before each input, including the first one, without changing the sender's adaptive controller state. AMF writes the byte budget as bits, filters increases of at most 10%, honors decreases immediately, and keeps driver readback distinct from requests. Each submitted picture retains that observed value through asynchronous output and packetization. Explicit `amd_max_frame_size` values, including `0`, win; Automatic owns its cap independently of bitrate rescaling. The initial encoder-settings log can still show the conservative 2× setup value; use the per-picture `video recovery frame` log for the submitted cap.

Confirmed loss is retained for the authenticated paired identity plus canonical IP for 24 hours, with four remembered reconnects before a fast-rate probe. Reset and unpair operations fence running sessions so their later teardown cannot restore erased history. A probe only clears the history it actually tested, and only after at least five seconds, 120 sent frames, no fresh latch and no stream failure. Adaptive FEC now holds 30 seconds and decays 1 percentage point per five seconds; the default 20→5% decay takes 100 clean seconds. The ordinary loss threshold, HEVC long-term-reference restriction, AV1 confirmation keyframe and Xbox intra-refresh policy are unchanged.

A task-owned adaptation of the earlier fixture is retained at `D:\CodexArtifacts\rubylight-keyframe-recovery-20261018\fixture`. It supports an explicit 2× versus Automatic ABBA comparison at both 70.77 and 800 Mbps, with requested-keyframe decode timing, receive gaps and the following ten frames. The installed-host preflight reported an active application, so neither the synthetic native cap test nor the stream comparison was launched. The application was left untouched. There is no new hardware latency or quality result here; the historical fixed-cap measurements above must not be presented as validation of this policy. The static-desktop fixture also lacks controlled reference pixels and cannot establish visual quality by compressed size alone.

## October 24: predictive claim waits under VRR

Under the VRR hint the pacer did not wait for a predicted fresher frame, because the 1000 Hz virtual display would make every claim wait for nothing. A game faster than the stream on a source that is not at 1000 Hz then lost 2-4 ms: the claim took the first frame the credit allowed, which was up to a game frame old. The pacer now has a prediction floor: under VRR it waits for a predicted frame when the learned source interval is at least 2 ms (`Pacer::with_prediction_floor`, `stream.rs`); below that it behaves as before, equal to prediction off (unit test `a_prediction_floor_leaves_a_1000_hz_source_unchanged_and_waits_for_a_slower_one`).

Measured on the RX 7900 XT (240 Hz physical display), HEVC, 2560x720, 60 fps stream, the VRR hint on, `minimum_fps_target=20`, a scripted game (`motion_probe`) at 120, 240 and 60 Hz, 25 s per run, the first 5 s dropped. A is the same host build with `frame_pacing_predictive=false` (the old VRR behaviour), B the default; order A B B A, two runs each, means of the two. Age is `capture_age_at_submit_ms` and present-to-sent `present_to_sent_ms` from the 5 s `stream timings` lines (two lines per run after the warm-up, so the p99 columns are noisy); the cadence error is `host_cadence_error` over the whole run (about 1200 pairs).

| game / leg | age p50 / p95 / p99 | present-to-sent p50 / p95 / p99 | cadence error p50 / p99 | shown fps |
| --- | --- | --- | --- | --- |
| 120 Hz, A | 4.68 / 9.70 / 10.91 | 7.27 / 12.11 / 13.49 | 0.69 / 9.15 | 60.10 |
| 120 Hz, B | 0.27 / 3.71 / 8.91 | 3.80 / 6.39 / 11.43 | 0.24 / 8.83 | 60.10 |
| 240 Hz, A | 2.53 / 4.25 / 5.00 | 4.76 / 7.02 / 7.59 | 0.46 / 4.71 | 60.10 |
| 240 Hz, B | 0.23 / 0.30 / 2.75 | 2.83 / 3.25 / 5.48 | 0.10 / 2.96 | 60.10 |
| 60 Hz, A | 0.26 / 0.32 / 0.38 | 2.81 / 3.26 / 3.33 | 0.14 / 0.73 | 60.00 |
| 60 Hz, B | 0.23 / 0.28 / 0.31 | 2.80 / 3.26 / 3.31 | 0.10 / 0.75 | 60.00 |

The individual runs agree: the 120 Hz cadence error p99 was 8.72 and 9.57 (A) against 8.64 and 9.02 (B); the 240 Hz one 4.83 and 4.58 (A) against 4.83 and 1.08 (B), so the 240 Hz p99 gain comes from one of the two B runs and is not claimed. Median age falls by 4.4 ms and present-to-sent by 3.5 ms at 120 Hz, by 2.3 and 1.9 ms at 240 Hz; the 60 Hz game and the shown fps do not move. The 120 Hz game keeps a 3.7 ms age p95: a predicted frame that does not arrive inside the budget is still claimed late. Kept.

Not measured: the 1000 Hz virtual display. An isolated host can only create that display as SYSTEM in the console session, and no harness for it exists here (`e2e.py` captures the physical display with `virtual_display_mode = disabled`). That the 1000 Hz case is unchanged rests on the floor: below 2 ms the pacer takes the same branch as with prediction off, which the unit test asserts.

## October 24: beam-raced PyroWave gate (B10 step 4, opt-in, missed its default-on gate)

Nightjar desktop, loopback to an isolated Rubylight host on this PC (RX 7900 XT), 1080p60 PyroWave 4:2:0
at 399 Mbps, release builds, borderless fullscreen on the 5120x1440 240 Hz VRR display, `beam_race` on,
`NIGHTJAR_STREAM_LOCATION=local`. The raced presenter presents each decoded slice before the display's
beam reaches its rows; the whole side is the same build with `NIGHTJAR_BEAM_RACE_PRESENT=0`. The
display's scan is S = 4.17 ms over 1440 rows, and a tearing present restarts it after a median 0.91 ms.
Short form of the gate: 60 s legs in ABBA order, two runs a side, the first 300 frames of each run
dropped. The age is the mid-row age: when the beam reaches the picture's middle row minus when the
slice holding that row was ready. The keep rule asks for a median and p95 gain of at least
min(W/2, S/2), where W is the frame's time on the wire at the pace (7.4 ms at pace 900, 3.17 ms at 2100),
and `receive_to_present` p95 no worse, late slices under 1%, no composed presents and no hang.

| Leg | Required | Age p50 whole / raced (gain) | Age p95 whole / raced (gain) | `receive_to_present` p95 whole / raced | Late slices | Frames raced |
|---|---|---|---|---|---|---|
| 1: pace 900 | 2.08 ms | 5.32 / 3.93 (1.40) | 5.98 / 5.33 (0.65) | 8.64 / 8.61 | 0.061% | 5301 of 6578 |
| 2: pace 2100 | 1.58 ms | 4.18 / 3.47 (0.71) | 4.79 / 4.19 (0.60) | 4.59 / 5.19 | 0.000% | 6344 of 6582 |
| 5: pace 900, 0.1% loss, one run a side | (leg 1's) | 5.30 / 3.91 (1.39) | 5.88 / 5.45 (0.44) | 8.33 / 8.53 | 0.015% | 2646 of 3292 |

All runs exited 0, with no dropped frames, no decoder errors and no hang; PresentMon saw the same
few composed presents on both sides. The rule fails on legs 1 and 2 (the median gain is 67% and 45% of
what it asks for, the p95 gain less), and on leg 2 `receive_to_present` is 0.60 ms worse at p95,
so it does not become the default. The race gains about 1.4 ms at the middle row
at pace 900 and nothing at the bottom: the last rows reach the screen later than a whole-picture present
puts them (a raced frame has 2.4 presents, the overlay costs about 0.2 ms each). A margin sweep was not run.
Merging a set's consecutive slices into one draw helped a little;
a raw `Present1` and skipping the flush did not move the numbers.

Caveats, both of which make the loopback read worse than a separate client would: the host's encoder and
the client's decoder and present share one GPU, and the host sends in batches of about 1 ms, so slices
arrive in steps. The wired separate-PC legs (the NUC at 1 GbE and 2.5 GbE, legs 3 and 4) and a windowed
leg were not run, so the wired case, which the idea is for, is untested. No claim beyond this PC's loopback.
The Nightjar desktop client keeps all of it (row-order request, scan line probe, slice decodes, raced
presenter) behind one experimental setting, `beam_race` (`NIGHTJAR_BEAM_RACE=1`), off by default; off, it
asks for no row order, starts no probe and decodes whole frames, and on a stream that cannot race it
says why in the log and on the stats overlay. The host's row order (`pyrowaveRowOrder`) stays in
`rust/core` behind the RTSP key, default off. Record: Nightjar `docs/pyrowave-decode.md`, "Beam racing:
measured, opt-in", and `docs/beam-racing.md`.

## October 26: beam-raced PyroWave over Wi-Fi 6E to the 780M laptop (B10, quick pass)

The first run on a separate client. Host: this PC (RX 7900 XT), installed 3.0.0-rc.2 with an isolated
profile on 192.168.4.69:48123, wired. Client: the 780M laptop (Ryzen 7 8845HS) on Wi-Fi 6E (RZ616,
5 GHz channel 116, 802.11ax, 2402 Mbps both ways, signal 98% before every run), Nightjar desktop release
build of 2026-10-25, GPU decode, borderless fullscreen on its 2880x1800 120 Hz panel, tearing on.
1080p60 PyroWave 4:2:0 at 399 Mbps (the host sent 339 to 341 Mbps), `NIGHTJAR_PYROWAVE_LINK_MBPS=2500`
and `NIGHTJAR_PYROWAVE_PACE_MBPS=900` (the client sees a wireless route and would not ask for the row
order without them), `NIGHTJAR_STREAM_LOCATION=local`. ABBA, 60 s legs, the first 300 frames of each
dropped. A is `beam_race` off (index order, whole decodes), B is `beam_race` on.

**It did not race.** The laptop's panel scans all the time: the probe measured 8.37 ms over 1800 rows
and 31 blanking lines (119.5 Hz), 1% of the readings in the blanking, and over 300 presents no present
found the display idle, so the verdict was "no VRR" and every frame was presented whole. The panel's
EDID has no refresh range in its base block. Racing needs a present to restart the scan, so this laptop
cannot measure the raced presenter as it is. What the run does show is the row order and the slice
decodes over real Wi-Fi:

| Leg | Row order | Frames | Row ordered | `first_slice_ready - receive` p50 | `last_slice_ready - receive` p50 | Late slices | `receive_to_present` p50 / p95 |
|---|---|---|---|---|---|---|---|
| 1 A | off | 3294 | 0 | | | | 8.36 / 9.63 ms |
| 2 B | on | 3294 | 100% | 2.44 ms | 8.38 ms | 0 of 13176 | 9.29 / 10.55 ms |
| 3 B | on | 3293 | 100% | 2.60 ms | 8.74 ms | 0 of 13172 | 9.64 / 10.90 ms |
| 4 A | off | 3294 | 0 | | | | 10.37 / 11.87 ms |

Pooled, `receive_to_present` p50 / p95 was 8.82 / 11.45 ms with the row order off and 9.46 / 10.77 ms
with it on. The two A legs differ by 2 ms at the median, more than A against B, so one ABBA set
shows no cost or gain from the row order on this link. Every leg exited 0, with no frames dropped at
the decoder and no early-decode fallbacks. Over Wi-Fi the first of the four slices was ready 0.8 to
0.95 ms later than on loopback (1.65 ms) and the last 1.2 to 1.5 ms later (7.20 ms), and no frame
broke the row order.

Not shown: any presented-picture gain. That still needs a client with a VRR display, on Wi-Fi or on
the wired NUC legs.

## October 26: beam-raced PyroWave gate rerun on loopback (B10)

Legs 1 and 2 of the [October 24 gate](#october-24-beam-raced-pyrowave-gate-b10-step-4-opt-in-missed-its-default-on-gate)
again, with the installed 3.0.0-rc.2 host (isolated profile on 127.0.0.1:48123) and the Nightjar desktop
release build of 2026-10-25 that the laptop run used. Same setup: RX 7900 XT, borderless fullscreen on the
5120x1440 240 Hz VRR display, 1080p60 PyroWave 4:2:0 at 399 Mbps (the host sent 318 to 353 Mbps), GPU
decode, early decode, `beam_race` on, `NIGHTJAR_STREAM_LOCATION=local`, the pace set with
`NIGHTJAR_PYROWAVE_PACE_MBPS`. ABBA, 60 s legs, the first 300 frames of each dropped; A is
`NIGHTJAR_BEAM_RACE_PRESENT=0`. The probe said "on" for every frame of every leg. No PresentMon this time.

| Leg | Required | Age p50 whole / raced (gain) | Age p95 whole / raced (gain) | `receive_to_present` p95 whole / raced | Late slices | Frames raced |
|---|---|---|---|---|---|---|
| 1: pace 900 | 2.08 ms | 5.49 / 4.32 (1.17) | 6.20 / 5.66 (0.54) | 8.34 / 8.67 | 0.080% | 3877 of 6924 |
| 2: pace 2100 | 1.58 ms | 4.72 / 3.77 (0.95) | 5.36 / 4.72 (0.64) | 4.68 / 5.66 | 0.023% | 6107 of 6944 |

Every leg exited 0, with no hang, no decoder fallback, no frame out of row order. The verdict stands: both
legs miss the rule at the median and at p95, and `receive_to_present` p95 is worse with the race at both
paces (0.33 and 0.98 ms). Against October 24, the median gain is 0.23 ms lower at pace 900 and 0.24 ms
higher at 2100, and the p95 gains are within 0.11 ms. At pace 900 fewer frames raced (56% against 81%):
the planner skipped 1928 frames as "pace leaves under 1 ms to gain". The whole side moves between legs
too (mid-row age p50 5.77 and 5.33 ms in the two A legs at pace 900), so one ABBA set resolves the sign
of the gain, not the 0.2 ms differences between the two runs. The first slice was ready 1.6 ms after the
frame's first packet at pace 900 and 0.75 ms at 2100, the last 7.0 and 3.3 to 3.6 ms.

## October 27: LTR recovery on H.264 and HEVC (1.4, Experimental, stays off)

The ignored native test `amf::ltr_native_tests::native_ltr_recovery_after_lost_frames`, release build of
branch `rl-ltr`, RX 7900 XT: a scrolling synthetic texture (no capture, display or network), 2560x1440 at
120 fps, 50 Mbps, 360 frames. Frames from 241 on are dropped for the gap, then the encoder is told (the
stock arm asks for a keyframe; the LTR arm, opted in with `rlLtrRecovery=1`, invalidates the lost frames).
Both streams are decoded with ffmpeg 9.0.2 and the frames compared by hash to the clean stream of the same
decoder. Bytes are the recovery frame; the stock IDR at that point was 82,213 B (H.264) and 90,801 B (HEVC),
the mean inter frame about 40 KB and 42 KB.

| Arm | Recovery frame | Bytes | Software decode | d3d11va and dxva2 |
|---|---|---|---|---|
| H.264 gap 1 | LTR P | 78,334 | identical | every frame after it differs |
| H.264 gap 3 | LTR P | 117,886 | identical | every frame after it differs |
| H.264 gap 7 | LTR P | 206,618 | identical | every frame after it differs |
| H.264 gap 8 | LTR P | 203,776 | identical | every frame after it differs |
| H.264 gap 11 | LTR P | 179,334 | identical | every frame after it differs |
| H.264 gap 15 | IDR (invalidation refused) | 82,276 | identical | identical |
| HEVC gap 1 | LTR P | 59,066 | identical | identical |
| HEVC gap 3 | LTR P | 90,680 | identical | identical |
| HEVC gap 7 | LTR P | 224,558 | identical | identical |
| HEVC gap 8 | LTR P | 254,555 | 222 errors, nothing after the loss | same |

No decoder logged an error on the H.264 LTR streams, but both hardware decoders showed a different picture
from the recovery frame to the end of the stream (118 frames at gap 1), so a Windows client with hardware
decode would keep a damaged picture until the next keyframe. HEVC at gap 8 decoded none of the 111 frames
after the loss on any decoder: a freeze. Only HEVC at gap 1 saved bytes (35% smaller than the IDR); from
gap 7 on the recovery frame is 2.2 to 2.8 times the IDR. The verdict: no improvement without freezes, so
LTR stays off by default, behind its Experimental launch parameter, and no Nightjar setting asks for it.

HEVC gaps 11 and 15 were not measured in that run: it stopped at the wrapper's 900 s limit while HEVC gap
11 was finishing (the test's own 2 s AMF stall check never fired; HEVC arms took 50 to 220 s). A rerun on
main `e2b24ff23` with 30 minutes froze both the way gap 8 did.

The freeze, fixed on October 28: AMF writes HEVC with a four-bit POC LSB
(`log2_max_pic_order_cnt_lsb_minus4 = 0`), and a decoder that never saw the lost frames derives the
recovery frame's POC from the last frame it showed. More than 8 frames on, it lands 16 too low (236
instead of 252 at gap 11), and the long-term reference, named by its MSB cycle from that POC, points at a
picture the decoder never had (FFmpeg: "Could not find ref with POC -16"). The October 10 HEVC branch in
[PERFORMANCE_WORK.md](PERFORMANCE_WORK.md) had found the same bound. The host now answers such an
invalidation with a keyframe and logs why; H.264 keeps its rule of one 16-frame frame_num period, and AV1
names its references by slot with an explicit order hint. Rerun on branch `rl-ltr` (`467894869`, same
test): HEVC gap 7 is still an LTR P frame (224,558 B), and gaps 8, 11 and 15 now get an IDR (90,634 to
91,305 B, the stock arm's was 90,633 B). Software, d3d11va and dxva2 show the full stream's pictures from
the recovery frame to the end on every arm, with no decoder errors. The slowest AMF call was 433 ms.

Not run: decodes on the NUC and the laptop. On the stock stream (before any loss), ffmpeg's
d3d12va decoder failed with "No space for new Reference frame" after about 17 frames, Vulkan with
`VK_ERROR_OUT_OF_DEVICE_MEMORY`, and `h264_amf` exited nonzero with no message, so those were left out.

## October 28: LTR recovery on H.264 and HEVC with header rewrites (on by default)

Moonlight and Nightjar drop every frame from a loss until the repair, so the repair always follows a gap.
The October 27 freezes came from that gap: HEVC's four-bit POC LSB puts the repair 16 too low after more
than 8 dropped frames, and FFmpeg's d3d11va and dxva2 H.264 decoders mishandle any frame_num gap. On a
stream with long-term references the host now rewrites AMF's headers for a client that skipped the lost
frames: HEVC gets an 8-bit POC LSB, every slice POC rewritten and explicit MSB cycles for its long-term
entries (repairs up to 128 frames after the last one shown, a keyframe beyond); H.264's frame_num is
renumbered to leave out the skipped frames. A stream without long-term references sends AMF's bytes
untouched. The host repairs only when its estimate (mean inter frame bytes times the anchor's age) is
under the last keyframe's size.

Same ignored native test as October 27, release build of branch `rl-ltr`, RX 7900 XT, 2560x1440 at
120 fps, 50 Mbps, 480 frames, frames from 264 on not shown for the gap. Two contents: the scrolling
texture (16 px a frame) and "static", the texture held still with a 64 px square crossing it 32 px a
frame. The full stream is AMF's own bytes, the lossy one the rewritten bytes without the gap. Decoded
with ffmpeg 9.0.2 software and d3d11va, frames compared by hash. Bytes are the recovery frame, forced
to repair (`-Force`) against the keyframe of a stream without long-term references.

Scrolling:

| Gap | H.264 IDR | H.264 repair | HEVC IDR | HEVC repair | AV1 keyframe | AV1 repair |
|---|---|---|---|---|---|---|
| 1 | 82,238 | 58,293 (0.71) | 91,319 | 50,700 (0.56) | 102,781 | 53,922 (0.52) |
| 2 | 82,240 | 79,971 (0.97) | 90,775 | 66,524 (0.73) | 101,678 | 57,822 (0.57) |
| 4 | 82,274 | 121,630 (1.48) | 90,719 | 93,115 (1.03) | 101,617 | 86,723 (0.85) |
| 8 | 82,235 | 215,735 (2.62) | 90,731 | 243,494 (2.68) | 101,555 | 247,386 (2.44) |
| 16 | 82,280 | 225,548 (2.74) | 90,758 | 233,271 (2.57) | 101,171 | 101,145 (1.00) |
| 64 | 82,251 | 236,916 (2.88) | 90,814 | 239,696 (2.64) | 100,366 | 100,340 (1.00) |
| 130 | 82,240 | 236,498 (2.88) | 91,000 | IDR, 91,056 (past 128 frames) | 99,157 | 99,131 (1.00) |

Static:

| Gap | H.264 IDR | H.264 repair | HEVC IDR | HEVC repair | AV1 keyframe | AV1 repair |
|---|---|---|---|---|---|---|
| 1 | 82,245 | 55,869 (0.68) | 90,787 | 52,703 (0.58) | 101,775 | 51,792 (0.51) |
| 2 | 82,294 | 46,816 (0.57) | 90,741 | 49,130 (0.54) | 101,780 | 48,255 (0.47) |
| 4 | 82,235 | 48,119 (0.59) | 90,805 | 57,076 (0.63) | 101,749 | 49,280 (0.48) |
| 8 | 82,237 | 53,331 (0.65) | 90,830 | 57,358 (0.63) | 101,618 | 59,826 (0.59) |
| 16 | 82,219 | 211,875 (2.58) | 90,885 | 236,050 (2.60) | 101,419 | 101,393 (1.00) |
| 64 | 82,270 | 215,780 (2.62) | 91,129 | 246,287 (2.70) | 99,161 | 99,135 (1.00) |
| 130 | 82,294 | 223,590 (2.72) | 91,194 | IDR, 91,196 (past 128 frames) | 99,163 | 99,137 (1.00) |

Decodes: all 84 forced arms and all 42 policy arms showed the full stream's pictures from the recovery
frame to the end on software and d3d11va, with no decoder errors (the first forced H.264 run compared
against the rewritten stream as its reference, which itself drops frames, and was rerun against AMF's
bytes). The offline streams of the morning, rewritten from recorded AMF output, also decoded identically
on d3d11va and dxva2: HEVC gap 8 111/111 and gap 11 108/108 frames after the recovery, H.264 gap 1
118/118, gap 8 111/111 and gap 11 108/108.

Policy run (no `-Force`, both contents): on every codec the estimate repaired at gaps 1 and 2 and sent a
keyframe from gap 4 on, so no repair it chose was bigger than the keyframe. It passed up smaller repairs
on static content at gaps 4 and 8 (0.48 to 0.65 of the keyframe) and AV1 scrolling at gap 4 (0.85). Over
the 14 gap and content cells per codec the policy sent 0.92 (H.264), 0.89 (HEVC) and 0.86 (AV1) of the
keyframe bytes, and 0.52, 0.62 and 0.97 of what repairing every gap would have. AV1 repaired every gap
before the estimate; its 3% saving here is a mix of avoiding the 2.4x repair at scrolling gap 8 and
missing the static ones. So long-term references are on by default for H.264 and HEVC as for AV1
(`amd_ltr_frames`, default 4), and the launch parameter `rlLtrRecovery` no longer changes anything.

Cost of the rewrite. Offline, release build (core ignored test `rewrite_time_per_frame`,
`BUTTERPOLLO_TEST_REWRITE`): rewrite p50 10 to 14 us, p99 37 to 62 us, against the copy the encoder already makes (p50
2.6 to 3.9 us, p99 19 to 44 us): about +9 us p50 per frame, only on streams with long-term references. In
a real stream, `rust/release/e2e.py` with the moonlight-common-c receiver, HEVC 2560x1440x120 at
50 Mbps, d3d11va decode, 60 s per side with the first 5 s left out, release host at `80221f3db`; the LTR
side asked with `rlLtrRecovery=1` (the rewrite on every frame), the other side was the stock stream, as
on main. ABBA, one stream each:

| Stream | Host processing, median of 5 s means | Median of 5 s p99 | Record host total median / p99 | End to end median / p99 |
|---|---|---|---|---|
| stock 1 | 0.483 ms | 1.218 ms | 3.401 / 4.787 ms | 5.445 / 6.843 ms |
| LTR 1 | 0.621 ms | 0.929 ms | 3.384 / 4.284 ms | 5.451 / 6.382 ms |
| LTR 2 | 0.616 ms | 0.948 ms | 3.414 / 3.737 ms | 5.497 / 5.891 ms |
| stock 2 | 0.627 ms | 0.945 ms | 3.400 / 3.721 ms | 5.475 / 5.883 ms |

The first stock stream's host processing rose to the others' level (0.62 to 0.65 ms) in its last three
windows; the second pair differs by 11 us in the mean and 3 us in the p99, and the record's host total
by 14 us in the median. The rewrite adds no latency we can measure in a stream. The 5 s line carries a
mean, p95 and p99, no median. All four streams held 120 fps with no decode failures; no loss was injected,
so no repair was sent in them.

Not run: decoders other than FFmpeg's on this PC (Android MediaCodec, Apple VideoToolbox, the NUC and
the laptop). A separate NUC run, HEVC with LTR, repaired 40 ms bursts and decoded 2215/2215 frames. Limits:
HEVC repairs only within 128 frames of the last one shown; the skipped-frame history keeps 64 repairs
per IDR; the size estimate is linear; the rewriters have mutation tests (3000 per codec), not a fuzz
harness. Android MediaCodec checked in the rc.4 hardware pass.

## Hardware covered

Measurements ran with AMD AMF on an RX 7900 XT host, with real-client Wi-Fi
runs to a Radeon 780M laptop recorded for
[2.2.9 on October 12](PERFORMANCE_WORK.md#real-wi-fi-hardware-receiver-streams-october-12)
and [rc.24 on October 8](PERFORMANCE_WORK.md#october-8-real-client-picture-age-over-wi-fi-rc24-laptop).
Native NVENC calls the installed NVIDIA driver directly and supports reviewed
API versions 11.0–13.0, reference frame invalidation, D3D11 4:2:0/8-bit 4:4:4
and GPU-only CUDA interop for ten-bit 4:4:4. QSV has native D3D11 imports, and
TrueHDR has a shared-device GPU path. PyroWave uses shared D3D11/Vulkan planar
GPU inputs and reads back only the encoded bitstream. Unsupported native
formats and software encoding use CPU compatibility paths. The GPU texture
pools and native encoder queues are bounded to eight retained frames.
[Features](../docs/features.md) lists everything the host does.
