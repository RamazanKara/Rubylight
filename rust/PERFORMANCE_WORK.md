# Windows performance work — October 2, 2026

User objective: make the Rust host smoother and lower latency than Vibepollo,
without reducing features or picture quality. Opus took over from Codex in the
evening of October 2.

This is a dated engineering log, newest entry first. Each entry records what
was and was not measured on its own date, so a "not measured" note in an older
entry may have been measured in a later one. Results to a real laptop over
Wi-Fi are in "October 8 real-client picture age over Wi-Fi"; the
[performance overview](../docs/performance.md) carries the current numbers.

## October 18 retired compute queues and conservative GPU priority

The stability changes reject new capture imports, cursor uploads, conversions,
handoffs and submissions after a shared compute queue has been retired. Old
completion fences remain usable for cleanup; the guard does not cancel calls
already in progress or wait behind a potentially stalled native call. Their
resource ownership and completion fences still apply. AMD, Intel and unknown
adapters now use process GPU priority class 4 (high), while NVIDIA's existing
HAGS/opt-in policy is preserved. A separate boundary check rejects an AMF input
texture state that cannot be transitioned on a compute command list before
advancing the conversion slot or recording GPU work.

**Validation.** On the RX 7900 XT, ordinary tests passed for GPU priority
(2), compute failure/state rules (3), timing (6), and input (63). Two opt-in
native regressions passed: a completed task-owned queue was marked retired and
could no longer accept cached cursor uploads, conversions, imports or handoffs;
unsupported AMF metadata was rejected without changing submission counters,
conversion slots or targets. Restoring valid metadata allowed normal conversion.
The tests neither forced device removal nor submitted invalid resource barriers.

**Before/after component measurement.** Ryzen 7 5800X3D, RX 7900 XT,
driver `32.0.32015.2008`; optimized Windows lib-test executables. Baseline
`ce3292ab195f97140aa3fc6507cb0b4270d2cf80` was built in a detached worktree.
The changed executable includes the retirement guards, resource-state guard,
priority policy and the concurrent display/input/timing work as of this build.
Compilers were stopped during measurement. The existing
`capture::idd_push::fence_tests::keyed_and_fenced_transport_abba_probe` ran in
baseline/changed/changed/baseline order. Each executable internally ran
keyed/fenced/fenced/keyed for BGRA8 SDR and RGBAF16 HDR at 2560x1440, paced at
120 fps. Each round used 8 warmups and 64 measured frames: 256 measured frames
per version/format/transport, 2,048 overall. Every round also verified the
captured input pixels by readback.

These are producer-copy-through-completed-conversion times, in microseconds.
The p95 columns give the range of four per-round p95 values, not a pooled p95.
All rounds are included.

| Format / transport | Baseline mean | Changed mean | Baseline round p95 range | Changed round p95 range |
| --- | ---: | ---: | ---: | ---: |
| SDR / keyed mutex | 580.286 | 536.052 | 671.1–1,551.2 | 731.0–821.9 |
| SDR / fence | 807.252 | 491.020 | 692.8–10,244.3 | 606.4–756.8 |
| HDR / keyed mutex | 578.152 | 572.439 | 743.2–835.3 | 726.4–859.6 |
| HDR / fence | 523.037 | 538.093 | 716.6–729.7 | 712.6–752.6 |

The final baseline SDR fence round had a large producer/copy tail, retained in
the results. Its producer submission mean was 1,106.488 us versus roughly
110–126 us in the other SDR fence rounds. This noisy short comparison does not
establish a latency improvement or a statistically resolved regression. It
does show both transports and formats continuing to convert and release
frames with the new admission checks. There was no encoder, game load, network,
receiver, installed driver/service operation or display change in the probe.
It does not exercise process GPU-priority configuration, and it cannot establish
that the long-running AMD video-engine resets are resolved.

Reproduce by building `cargo test -p butterpollo-windows --lib --release --locked
--no-run` in each source tree, saving each executable separately, then running
the named probe with `--ignored --exact --nocapture --test-threads=1` in the
sequence above. Switching source trees through a shared target directory
required clearing the three local release crates (core, vdisplay-proto and
windows) to avoid reusing stale path-package artifacts. The task target was
`D:\CodexArtifacts\rubylight-four-fixes-20261018\target`, with four build jobs.
Saved evidence is in the sibling `gpu` directory: both test executables,
`run-probe.ps1`, all four raw logs, `measurements.csv`, `summary.csv`, native
test logs, build diagnostics, hardware details, the GPU source diff and full
SHA-256 identities. Binary hashes are
`7b0c94f4907aa063edc8bda879d04701f855f46b806920be0d65bac65f0725a6`
(baseline) and
`c7c0d013d88e18c767ec294607db4fad4582a27d98ab99de453f3b231cd65889`
(changed); the GPU source-diff hash is
`2ccee38710e32af796384f298547ec22aea9e70d1bfe72d913b5681d9e6564d2`.

**Follow-up API review.**

- AMF surface release ownership stays keyed by the live native surface.
  [AMF's Surface.h](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/public/include/core/Surface.h)
  specifies that the observer runs before internal resources are released.
  The wrapper owns the returned surface reference and keeps the boxed observer
  alive through component/context termination. A delayed callback naming an
  already-destroyed, recycled allocation would violate that lifetime contract;
  no such runtime behavior was reproduced. The comment now describes the
  actual contract instead of claiming arbitrary duplicate callbacks are safe.
- [AMF's D3D12AMF.h](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/public/include/core/D3D12AMF.h)
  defines the private resource-state value, but does not make every possible
  state legal on our queue. The
  [DirectX queue-state matrix](https://microsoft.github.io/DirectX-Specs/d3d/CPUEfficiency.html#state-support-by-command-list-type)
  requires both sides of a transition to be supported. The new check accepts
  COMMON, UAV, COPY_DEST, COPY_SOURCE, NON_PIXEL_SHADER_RESOURCE and the legal
  combined read state; it rejects graphics/video-only states, unknown bits and
  mixed read/write masks. The error follows existing encoder recovery rather
  than silently assuming COMMON. This is boundary hardening; the experiment
  did not establish that the AMD runtime returns a video-only state here.
- The AMF compressed-output copy remains necessary: `GetNative` lends bytes
  from the AMF buffer, which is released before asynchronous sending. `Encoded`
  owns its `Vec<u8>` independently of that buffer. Removing `to_vec()` alone
  would leave the sender referencing storage AMF may reuse; a different buffer
  ownership design would require separate implementation and measurements.
- [D3D12CreateDevice's documented singleton behavior](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-d3d12createdevice)
  returns failure when the existing device is removed. The queue cache already
  rejects retired queues and removed-device fences; device creation was not
  changed on the unsupported premise that it returns a healthy-looking removed
  singleton. Capture recovery already releases frames outside its state lock.

## October 15 RX 7900 XT video engine resets: no host trigger found

Question: what makes the RX 7900 XT's driver reset during tests and streams.
Read-only: dumps, Windows events and logs. No stream, no forced hang.

**Dumps.** Every `C:\Windows\LiveKernelReports\WATCHDOG` dump on this PC
(Oct 9-14) read with cdb `!analyze -v` and public symbols. Each incident is
the same chain:

1. `0x141 VIDEO_ENGINE_TIMEOUT_DETECTED`, raised by
   `dxgmms2!VidSchiCheckHwProgress` → `VidSchiResetEngine`: one engine
   stopped, Windows tried an engine reset. Owner tag `amdkmdag+0x1d0fb0` on
   32.0.31041.1004 (Adrenalin 26.10.41.01), `amdkmdag+0x1c02d0` on
   32.0.32015.2008 (26.20.15.02, installed October 10 15:52). The dxgkrnl TDR
   record has the same fixed fields in all eight; the AMD part (`AMD0AMD1`,
   compressed) only AMD can read, and its log names `butterpollo.exe` at the
   second the host's encoder stalled.
2. `0x193 VIDEO_DXGKRNL_LIVEDUMP` reason 0x815 (watchdog) or 0x810
   (`ProcessDeadlockThread`) 10 s to 2 min later: a thread stuck in the driver.
3. `0x1B0 VIDEO_MINIPORT_FAILED_LIVEDUMP` "Add device failed",
   `STATUS_UNSUCCESSFUL`, then on two days "Start device failed": the adapter
   never came back. Code 31 until a reboot, every time.

Windows Error Reporting has the same chain since July (0x141 on July 9, 11,
12, August 22, September 3, 26, 30, October 6 and 8). The July to September
ones predate the installed Rust host; on September 26 and 30 the installed
host was Vibepollo with its C++ AMF path (same phone, AV1 HDR 1968x2184 at
120 fps, LTR 0, 1.7-7.7 Mb/s).

**Incidents** (local time; host logs; 0x141 dump written 4-6 s after the
encoder stopped):

| Dump | Host, case | Client | Codec | Mode | Mb/s | LTR | Recovery before | VRR | Capture | Display |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 10-09 02:47 | test, job 7 base run 5 | probe | HEVC HDR | 1968x2184@120 | 63 | off | none, 19 s in | off | DDX | virtual |
| 10-09 23:33 | test, HEVC recovery soak | probe | HEVC SDR | 5120x1440@120 | 63 | off | 2 keyframes/s | off | WGC | physical |
| 10-10 09:21 | test, c8-h264-5-A | probe | H.264 SDR | 2560x1440@120 | 63 | off | 80 ms outage every 1.5 s, 5 s in | off | WGC | physical |
| 10-10 10:20 | test, HEVC LTR branch | probe | HEVC SDR | 2560x1440@120 | 63 | on (4) | none, 6 s in | off | WGC | physical |
| 10-10 14:00 | installed | Legacy Moonlight | HEVC HDR | 3840x2160@120 (19 fps game) | 89 | off | none, 24 min in | off | WGC | virtual |
| 10-11 17:20 | a test host; no log survives | ? | ? | ? | ? | ? | ? | ? | ? | ? |
| 10-13 01:03 | installed | lap (Warhammer 3) | AV1 HDR | 2880x1800, ~120 fps | 69 | on (4) | none in the last minute, 100 min in | off | WGC | virtual |
| 10-14 08:41 | installed | SM-F966B | AV1 HDR | 1968x2184@120 | 399 | on (4) | none, 10 s in | on | WGC | virtual |

`MultiHwInstanceEncode` read 1 (driver default) in every HEVC and AV1 row;
H.264 has no such property. Every stream converted on the D3D12 compute
queue at high priority, with the GPU process at realtime priority. On
October 14 the client lowered the bitrate to 319 Mb/s 3 s after the encoder
stopped, with the hung SubmitInput already under way; no other row had a
bitrate change.

**Common factors.** Varied across the incidents, so not required: codec
(H.264, HEVC, AV1), HDR, LTR, bitrate (63-399 Mb/s), keyframe bursts, VRR,
capture (WGC, DDX), virtual or physical display, game or test pattern, the
split-frame property. Common to all: AMF encoding on this RX 7900 XT at 120
fps class rates, on two drivers and two host code bases. In the two test runs
that logged the probe (09:21, 10:20) the GPU's trivial query kept answering
while the encoder stalled, so the video engine hung alone; in the installed
host's three the query also waited ~3 s (and the audio thread 3 s in the
23:33 soak), which fits the engine reset pausing the GPU, not a 3D hang.
The installed host log (October 2-15, 14.5 h of AMF streams, 3.9 M frames)
has three resets: 2 in 3.2 h of AV1 with LTR, 1 in 11.3 h of everything
else, not significant with three events (p ≈ 0.12), and the test runs above
hung with LTR off and on H.264. Confidence: high that
none of LTR, AV1, HDR, VRR, bitrate changes or keyframe bursts is necessary;
medium that the trigger is in the driver or firmware; open whether the
realtime GPU priority or the split-frame default raise the rate.

**Ending the stream early does not help.** The research runner killed the
09:21 and 10:20 runs about 1 s and 6 s after the encoder stalled; both still
went 0x141 → 0x193 → 0x1B0 → Code 31. A thread blocked in SubmitInput cannot
leave the driver before the reset (the hung calls returned
`DXGI_ERROR_DEVICE_RESET` after 33.8-34.2 s), terminating AMF from another
thread would race that call on the same component, and AMF Terminate took
6 s on the reset GPU. Shipped instead: `rust/release/gpu_hang.py` stops e2e
and soak runs at the first hang evidence so `check.ps1` does not retry onto a
resetting GPU (see the release README).

**No default changed.** Split-frame has no measured benefit here (October 7:
one engine per stream whatever the property; six of 630 runs stalled, all with
the property on, p ≈ 0.09) but H.264 hangs without it. AV1 LTR shortens a
loss stall by 4-7 ms and shrinks the recovery frame from ~100 to ~20 KB
(October 10), and the hangs also happen without it. Proposed guarded A/B,
after a driver change is tried: alternate 30-minute AV1 1968x2184@120 HDR
blocks at 80 Mb/s (a) as now, (b) `amd_split_frame=disabled`, (c) GPU process
priority high instead of realtime, under `gpu_hang.py`, stopping at the first
hang and rebooting; at about one hang per 5 h of streaming it takes tens of
hours per arm to see even a threefold difference.

## October 13 adaptive AMF backlog candidate

The implementation in 2.2.12 adapts the application-depth learning rule from
[Vibepollo #461](https://github.com/Nonary/Vibepollo/pull/461) to the Rust host,
with timed admission to preserve conversion/encode overlap. Windows testing
compared current main `ff74b3fcb7ed7c1d89ac65cf91b97c136fe9360e` (2.2.11)
against refinement `bf009268a092fa55cf3e2c17e9b840556fd33f79`. Main had only
documentation changes since the original `dc02f1a9` baseline. The Rust
implementation shipped here is identical to the tested refinement.

The old host admits two pictures across encoding and conventional packet
sending. The candidate separates the encoder limit from that combined budget:

- Without pre-analysis, allow up to four accepted inputs until output shows
  the driver's retained depth. A fully drained pipeline learns a limit of one.
  When the first output leaves one accepted input, give it one bounded chance
  to return (at most 10 ms); a driver needing two inputs resumes after that
  probe. After depth one is demonstrated, admit the next input near predicted
  output, reserving conversion time and two 1 ms poll allowances. The fastest
  valid accepted-input-to-output sample bounds that delay, so temporary GPU
  load cannot lengthen it. Fast encoding can reduce the delay to zero.
- Preserve initialized pre-analysis lookahead. The configured depth of one
  starts with six slots and cannot learn a limit below two. Unavailable or
  out-of-range driver readback preserves the old fixed limit of two.
- Learn only from nonempty output matched to an accepted submission. Failed
  submissions, empty polls, expired timestamps and ownership callbacks do
  not count as completed pictures. Recreating the encoder restarts learning.
- Keep the combined allowance at least two so sending the previous packet
  can overlap encoding. Expand it only for larger encoder requirements.
  Sender pressure must not start encoder-stall recovery.
- Keep AMF's configured driver queue, eight-surface ownership cap, conversion,
  capture and polling behavior. A blocked claim polls output and then rereads
  the latest captured image; it does not mark an unsubmitted image as encoded.

### Radeon measurements and limits

Optimized Rust 1.98.1 GNU release builds ran serially in repeated ABBA order
with fresh profiles on port 48523. Hardware was an RX 7900 XT, driver
32.0.32015.2008 (September 28, 2026), with an Odyssey G93SC at 5120x1440,
240 Hz, SDR. Normal-rate fixtures used an extended virtual display: H.264
1920x1080 SDR or AV1 1968x2184 HDR, with source refresh matching requested
60/120 FPS. AMF used ultralowlatency/speed, VBR latency, driver input queue
four, compute conversion, VBAQ on, pre-analysis/SAV/HRD/LTR off. The saturated
HEVC fixture requested 150 Mbps; H.264 used 20 Mbps and AV1 HDR 80 Mbps.
Loaded runs used the same offscreen 2560x1440 graphics workload at 59 FPS.

Each stream ran 35 seconds with five seconds of warmup. An independent
D3D11VA receiver decoded picture IDs and QPC timestamps and checked them
against the source ledger. Picture age includes local decode and barcode
readback, but excludes presentation, scanout and remote-network latency.
These are means of per-run statistics, not pooled percentiles or game results.

| DDX condition | Runs per arm | Baseline mean / p95 / p99 age (ms) | Timed mean / p95 / p99 age (ms) | Fresh FPS, baseline / timed |
| --- | --- | --- | --- | --- |
| 5120x1440 HEVC, 240 FPS request, saturated | 4 | 19.083 / 21.130 / 21.711 | 16.439 / 18.478 / 19.261 | 219.830 / 219.190 |
| AV1 HDR, 120 FPS, GPU load | 4 | 11.355 / 15.529 / 18.319 | 11.196 / 15.457 / 18.037 | 119.631 / 119.475 |
| H.264 SDR, 60 FPS, idle | 8 | 5.636 / 7.325 / 8.200 | 5.745 / 7.034 / 8.116 | 59.999 / 60.001 |
| H.264 SDR, 60 FPS, GPU load | 8 | 9.103 / 13.764 / 15.459 | 9.408 / 14.013 / 16.375 | 60.001 / 59.995 |

Both saturated ABBA blocks improved mean age (2.687 and 2.602 ms); fresh FPS
fell 0.29% overall. Backlog logs confirmed learning from four to two to one.
The original one-input candidate had reduced saturated fresh FPS from 222.142
to 193.012, which motivated timed overlap. A separate fixed-three-input
control increased mean age from 19.076 to 23.705 ms with only 219.509 to
219.755 fresh FPS, so it is not included.

**The normal-load non-regression gate remains inconclusive.** Production
H.264 loaded p99 was 0.916 ms higher across four ABBA blocks. A separate
timing-logging copy (`5978455ddea19fb063477f0b804640f623cd0e2e`) reached zero
admission delay before warmup in all four sessions; its two ABBA p99 deltas
were +0.527 and -0.704 ms. Identical-baseline controls had +0.094 and -0.316 ms
deltas. This does not establish that the earlier higher tails are harmless,
nor justify a general normal-load latency improvement claim. DDX H.264
120 FPS idle also completed, with broad baseline-tail variation.

Eight saturated motion/static/IDR streams decoded all 256 requested keyframes
and retained the moving-picture age benefit after a two-second source pause.
Resume-to-first-fresh-picture ranged from 12.25–16.45 ms on baseline and
13.23–18.28 ms on the candidate; those four samples per arm do not clear a
recovery-timing regression concern. No unexpected encoder reset or decoded
ID/timestamp mismatch occurred in the valid completed runs. The installed
service was not changed; temporary display/HDR changes were restored.

All 835 ordinary Windows workspace tests, formatting, all-target Clippy with
warnings denied, and optimized builds passed. Fifty-two environment/native
tests remain ignored in the ordinary suite. Separate native HDR metadata,
reference-loss recovery and hardware-device-without-output tests passed on
both arms. The CI reconnect frame-count failure passed unchanged on rerun and
locally on both arms; its synthetic path does not exercise native encoding.

The full H.264/HEVC HDR/AV1 HDR 60/120 FPS DDX/WGC idle/load matrix, eight-bit
pre-analysis, native reconnect churn and injected encoder-stall matrix remain
incomplete. Earlier candidate WGC results are not validation of this timed
refinement. No real-game or remote-client performance claim is made. The
release includes the measured saturated improvement with these unresolved
limits; it does not claim that the original complete non-regression gate
passed, or remove the DWM/producer dependency from the October 12 investigation.

Raw ledgers, receiver CSVs, executable hashes, plans, logs and a source bundle
are retained on the test desktop under
`C:\Users\ramaz\Documents\Codex\2026-10-13\continue-rubylight-s-adaptive-amf-backlog\evidence`.
`decision.json` records the initial hold; the later release instruction
supersedes its publication decision, not its measurements or limitations.

## October 12 latency follow-up (issue 24)

[Issue #24](https://github.com/RamazanKara/Rubylight/issues/24) reviews 2.2.7
(`7ad9596`). Its timing estimates and Linux models identify candidates; they
are not Windows measurements. This pass implements the correctness and bounded
latency follow-ups and records the experiments still requiring evidence.

| Item | Disposition |
| --- | --- |
| R1: reconnect GPU scheduling | Implemented. The first streaming scope reapplies the last successfully configured GPU priority, including when capture is reused. The final scope still restores the prior class. |
| R2: host feedback | Implemented. Gamepad motion/rumble/light/trigger reports and changed HDR metadata signal a connected loopback socket in the control wait. Reports are queued before the wake; no periodic wake was added. The driver's existing 8 ms feedback poll remains. |
| R3: ENet backlog | Implemented. A pass that consumes its 512-event allowance uses a zero wait, so ENet's internal queue drains without an extra feedback interval between passes. |
| Active display leases | Implemented. Game and retained remote-monitor leases check every 100 ms while a stream owns them, retaining 250 ms idle checks and existing faster recovery retries. Resume unparks the workers. |
| Display watcher completion | Implemented. Journal ownership is released before waiting up to 100 ms for exit. A stalled owned watcher is terminated with a further bounded 100 ms wait; its completion event cannot be reused until exit is confirmed. |
| B1: WGC helper HIGH class | Implemented after controlled idle/loaded comparisons. Only the helper process moves to HIGH; its capture thread runs at priority 14 instead of 9, distinct from MMCSS 18. The results and the initial idle outlier are recorded below. |
| B2: Wi-Fi pacing at 4× | Deferred. The model predicts earlier packet delivery, but larger bursts can increase access-point/client loss. Keep 2× until real Wi-Fi ABBA runs, outage recovery and a ten-minute loss comparison establish the trade-off. No adaptive fallback was added without those measurements. |
| B3: newest image after encoder rebuild | Implemented. A successful rebuild rereads the latest image, verifies device compatibility and refreshes source timing before encoding it. A picture published during initialization can replace the pre-rebuild picture. |
| B4: static-desktop helper repaint | Implemented. The initial repaint deadline is 33 ms rather than 250 ms; retries stay 500 ms apart. This changes a deadline, not a measured launch-time result. |
| B5: interrupt a sending delta for an IDR | Deferred after prototyping and testing. Cancellation/nonce model tests passed, but the live recovery check did not trigger in-flight cancellation and did not consistently improve IDR p95. Existing pending-delta dropping remains. |
| B6: standing microphone backlog | Deferred after prototyping and testing. The queue/device model recovered within the proposed deadline, but baseline and candidate both failed the live tone probe, which never exercised trimming. The shorter device-buffer margin and speech-splice quality remain unvalidated. Microphone playout retains its existing behavior. |
| B7: final claim millisecond | Implemented. When collection is no longer attempted inside the final millisecond, the loop precisely waits on the claim deadline instead of repeating short non-precise output polls. |
| B8: hybrid WGC arrival/poll | Deferred. Explain the earlier excluded zero-frame run and obtain valid idle/loaded comparisons before changing capture wake policy. A theoretical earlier wake does not establish loaded scheduling behavior. |
| B9: paired-state save at startup | Implemented. Client last-seen persistence runs on a background worker with the existing paired-state lock; timestamps remain monotonic across overlapping starts. A failed save or worker start is logged. |
| B10: capture during `/launch` | Deferred. No first-frame measurement yet justifies the additional ownership path for abandoned launches, reconnects and differing negotiated modes. It also requires proving unused helpers are reclaimed within the proposed deadline. |
| B11: encoder backlog of one | Deferred. The issue predicts a throughput reduction. Keep the existing bound until the saturated fixture establishes the quality/throughput trade-off; no new configuration option was added. |
| B12a: release replaced capture image | Implemented. The old image is dropped after the publication lock is released and before notifying consumers. |
| B12b: grid pacing | Deferred. Guarding collection near the existing opt-in grid deadline and switching its waits to precise waits still need frame-age, jitter and CPU comparisons on that path. |
| B12c: parallel audio/display preparation | Deferred. No launch-stage measurement isolates its benefit, and concurrent preparation needs explicit failure, rollback and session-ownership validation. |
| B12d: skip pacing for short tails | Deferred. Changing the last batch's pacing is a burst-size trade-off; the arithmetic estimate is not evidence of a real-link gain without increased drops. |
| B12e: audio idle waits | Implemented. Event-driven capture waits account for the 50 ms silence threshold as well as the next packet deadline, avoiding an expired packet deadline forcing repeated 1 ms waits before silence is due. Capture events still wake immediately and the existing 5 ms wait cap remains. |

The issue's **measure-before-building** items remain experiments:

| Candidate | Evidence still needed |
| --- | --- |
| GPU handoff fences/keyed mutex and helper conversion | Per-hop GPU timestamps and an equivalent end-to-end comparison; the proposed sub-millisecond gain is unmeasured. |
| Separate encoder-output thread | Show that encode time exceeds the useful claim window and that another thread reduces picture age without changing throughput or ordering. |
| Drop the AV1 RFI confirmation keyframe | Establish decoder recovery correctness on independent clients before removing it. |
| Shorter hotplug quiet window | Measure reconnect/reconfigure tails and prove competing topology changes settle safely before shortening the current 500 ms window. |
| Cached WGC HDR output detection | Verify real HDR toggles through the cached output before changing cache or capture behavior. |
| Drain VHF feedback FIFO | Establish whether the driver actually queues reports and how reads complete. The new control wake does not change driver polling semantics. |

For **C, measurement gaps**, debug logs now record AMF encoder Init,
encoder/context Terminate and helper connection/Ready elapsed times. A launch
mode that differs from the negotiated mode emits an explicit display-preparation
log. There is no new preparation counter, and `interop.py`/`recovery_gaps.py`
have not been extended to aggregate all these stages. Launch-to-first-frame,
reconnect, reconfigure switch time, HDR-toggle freeze and microphone end-to-end
delay still need the issue's dedicated fixtures; log instrumentation alone
does not close those measurement gaps.

The paths checked as sound in section D are preserved. Section E's excluded
directions are not reopened: no slice/tile protocol, FEC-layout, wired-ceiling,
Opus-complexity, microphone reorder-buffer or cross-reopen D3D-device change.

### Validation

Locked Windows workspace tests passed: 815 tests, zero failures and 51
explicitly ignored native/environment fixtures. Workspace/all-target Clippy
with warnings denied, formatting, four release-tool tests and the 18-page
documentation-site build passed. The native RX 7900 XT streaming-scope test
passed, including a reconnect that reuses the still-live capture and restores
the original GPU class afterwards.

An initial HEVC 1920x1080x60 helper-forced ABBA compared released 2.2.7 against
the candidate that included HIGH helper priority, using a 5120x1440@240 SDR
source and twelve-second streams. All four runs decoded every received frame,
retained continuous audio and passed the existing stream checks. Baseline
picture-age means were 13.420/13.154 ms; candidate means were 21.683/17.932 ms.
Those values are outside the proposed idle tolerance, but a concurrent release
test build in another workspace was subsequently observed. These runs are
therefore unsuitable for latency acceptance or causal attribution. The priority
change was removed for the subsequent controlled comparisons. An earlier fixture attempt used relative paths
and exited before connecting; the recorded comparisons use absolute paths.

After the other release task finished, B1 was isolated on one candidate binary:
only the initialized helper's process class changed, with
`wgc_helper_streaming_scope=false`. HEVC 1080p60 at 20 Mbps used a 120 fps motion
source on the same 5120x1440@240 SDR display and a loopback D3D11VA receiver.
Source, receiver, tone and orchestration stayed HIGH in both arms to protect
the fixture from the CPU load. Independent 30-second launches ran in ABBA
order, discarding five seconds after priorities/load were established.
`input_path_probe --spin=highest` supplied one busy HIGHEST thread per logical
CPU in a NORMAL-class process. No other test or build ran concurrently.
Picture age means source barcode timestamp to barcode extraction after decode,
not presentation or input-to-photon delay.

| Condition / run | Helper | Fresh FPS | Mean age, ms | p95, ms | p99, ms |
| --- | --- | ---: | ---: | ---: | ---: |
| Idle A1 | NORMAL | 60.020 | 10.050 | 10.601 | 10.760 |
| Idle B1 | HIGH | 60.000 | 11.769 | 19.063 | 19.240 |
| Idle B2 | HIGH | 60.001 | 10.311 | 10.845 | 11.097 |
| Idle A2 | NORMAL | 59.999 | 10.408 | 10.893 | 11.168 |
| Loaded A1 | NORMAL | 40.449 | 44.373 | 63.597 | 81.423 |
| Loaded B1 | HIGH | 59.919 | 9.425 | 10.091 | 10.426 |
| Loaded B2 | HIGH | 59.997 | 10.785 | 11.278 | 12.387 |
| Loaded A2 | NORMAL | 41.365 | 48.336 | 70.942 | 96.332 |

Both loaded HIGH runs passed the stream checks and retained over 99% of idle
fresh FPS; both loaded NORMAL runs failed the moving-picture continuity check.
All runs had zero decode errors. These are trimmed, post-control windows:
loaded HIGH B2's broader E2E window still recorded a 552.240 ms picture-age
p99, so the comparison does not establish elimination of startup/load
transients. Independent idle launches averaged 0.810 ms
higher at HIGH, outside the proposed 0.3 ms tolerance. That outlier remains
unexplained, so a follow-up held the source, helper and receiver connected in
one 140-second stream and changed only helper priority in four 30-second ABBA
windows, again discarding five seconds after each assignment:

| Helper | Mean age, ms | p99, ms | Fresh FPS |
| --- | ---: | ---: | ---: |
| NORMAL A1 | 10.039885 | 10.813 | 60.001 |
| HIGH B1 | 10.053541 | 10.962 | 59.999 |
| HIGH B2 | 10.076316 | 10.917 | 60.001 |
| NORMAL A2 | 10.096460 | 10.839 | 59.999 |

The HIGH-minus-NORMAL mean was -0.003244 ms; adjacent comparisons were
+0.013656 and -0.020144 ms. All 8,378 received frames decoded with continuous
audio. This controls reconnect state and passes the idle criterion within
this stream. It does not explain the independent-launch outlier or establish
equivalence across sessions. B1 is accepted for the observed CPU-contention
benefit with that uncertainty recorded; no real-game, Wi-Fi or other-hardware
latency claim follows from these tests.

The combined candidate then passed four quiet 30-second HEVC 1080p60 streams
against released 2.2.8 in ABBA order, with continuous audio, no decode errors
and 59.999–60.000 fresh FPS. Mean picture age was 10.329/9.859/10.166/7.499 ms;
p99 was 11.192/10.650/11.126/11.732 ms. These independently launched sessions
do not establish an overall latency improvement.

H.264 1080p60 recovery at a 40,000 kbps pacing cap delivered and decoded all
20 requested IDRs in both builds, with no decode errors or unchanged pictures
re-encoded. An earlier quiet pair had baseline/candidate IDR-arrival p95 of
39.236/36.427 ms; the final combined pair measured 38.303/40.993 ms. Per-frame
and request CSVs found no missing wire IDs in any request-to-IDR bracket.
Candidate brackets also had no missing source barcodes, but sender debug logs
recorded no in-flight cancellations. Thus neither cancellation behavior on a
real paced stream nor a consistent recovery gain was established. B5 and its
prototype test were removed before release; no sender behavior changes ship.

The unshipped microphone prototype passed a deterministic queue/device model
with 20/40 ms network holds at 5/10/20 ms packet cadence: mean delay returned
within 5 ms of baseline by 900 ms without the hard queue limit or added silence.
Quiet native ABBA runs nevertheless detected only 36/59, 36/59, 37/59 and
36/59 tones, with zero trimming in every run. The existing `mic_probe` generates
tone pulses but has no `--burst` network option. These failures also occur in
the baseline; they neither establish a regression nor validate the proposed
drain. The prototype and its model tests were removed from the release.

After removing both prototypes, the rebuilt release passed another 30-second
HEVC stream and H.264 stream with 20 keyframe requests at the same settings.
Both decoded every received frame with zero decode errors and continuous
audio; all 20 recovery IDRs decoded, with no unchanged picture re-encoded.

## October 12 4:2:0 and pacing follow-up

The accepted GPU change uses one compute dispatch for D3D11 NV12/P010 and
PyroWave 4:2:0 fallback conversion, retaining pixel passes when resources are
unavailable. Default AMD D3D12 conversion is unchanged.
[The conversion evidence](PERFORMANCE.md#october-12-single-pass-d3d11-420-conversion)
records GPU timestamps, exact pixel comparisons, debug-layer validation and
independent stream checks. These establish component performance and
interoperability, not whole-stream latency or CPU savings.

Two pacing candidates remain unshipped. Timer-only waits for non-final batches,
preserving the precise final-batch wait, consistently increased sender
completion tails in the first ABBA matrix. At
150 Mbps/60 fps with an 800 Mbps wire target, final-packet p95 increased by
0.512 ms and keyframe p95 by 1.769 ms. At 80 Mbps/120 fps with a 160 Mbps wire
target, the increases were 0.100 and 0.350 ms. CPU results varied across runs;
there is no universal CPU-saving claim. The timer-only candidate was rejected.

The burst candidate sent each frame's second batch early, retaining its pacing
debt. An SM-F966B phone on the same Wi-Fi connection compared 2.2.7 with the
combined GPU-plus-burst candidate, using the
same moving scene, 1920×1080/60 HEVC SDR and 62,988 kbps negotiated encoder
rate. Each stable window lasted approximately 120 seconds. API counter deltas
and raw per-frame traces gave:

| Measurement | 2.2.7 baseline | GPU + burst candidate |
| --- | ---: | ---: |
| Host sent FPS | 60.371 | 60.369 |
| Video payload Mbps | 67.746 | 74.351 |
| Send duration mean / p95, ms | 0.731 / 1.603 | 0.823 / 1.613 |
| Client-reported inferred unrecoverable frames | 0 | 25 |
| IDR requests | 0 | 17 |
| Reference invalidations | 0 | 83 |

FEC reports are best effort, so zero reports do not prove zero loss. Adaptive
FEC varied in both windows, and measured payload rates were unequal. This
single pair does not prove causality: it combined GPU and pacing changes, and
wireless conditions vary. It nevertheless fails the no-regression gate, so the
burst change is dropped.

A separate GPU-only Wi-Fi check then completed in ABBA order: 2.2.7,
2.2.8, 2.2.8, 2.2.7. The same Radeon 780M laptop used official Moonlight
6.2.0 over WLAN, with identical 1920×1080/60 HEVC SDR settings, 80 Mbps
requested and 62,988 kbps negotiated. The host used
`gpu_compute_conversion=false` and the same full-screen 120 fps motion
scene. Each client run lasted approximately 165 seconds; host counters and
raw send traces cover an interior 120-second requested window. The GPU-only
build contains no pacing changes.

| Run | Host sent FPS | Video payload Mbps | Send duration mean / p95, ms | Send gap p95, ms |
| --- | ---: | ---: | ---: | ---: |
| A1: 2.2.7 | 59.999 | 74.881 | 0.881 / 1.656 | 18.076 |
| B1: 2.2.8 | 60.003 | 75.556 | 0.942 / 0.972 | 17.228 |
| B2: 2.2.8 | 60.000 | 75.736 | 0.937 / 0.980 | 17.712 |
| A2: 2.2.7 | 59.997 | 75.513 | 0.940 / 0.972 | 17.224 |

All four host windows had zero IDR requests, reference invalidations, frame
replacements, sender drops and send gaps above 1.5 frame periods. Client
statistics reported 60.04–60.06 incoming/decoded fps, 59.99 rendered fps,
0.00% network drops and 0.03% jitter drops in every run. Average reported
host latency was 2.4/2.4/2.6/2.5 ms in run order. This short check found no
observed regression at this workload; it does not establish exact packet
loss, because client percentages are rounded and no FEC block reports were
received. A2 matches the candidate's send p95, so A1's wider distribution
does not establish an improvement. No end-to-end latency or CPU saving is
claimed.

The laptop host evidence is in the same fixture directory below, in runs
`run-20261012T015057-514555Z`, `run-20261012T020813-526021Z`,
`run-20261012T021320-288379Z` and `run-20261012T021915-058867Z`, each with
`measurement.json` and a `host-logs` snapshot including rotated traces.
Client evidence is `a1`/`b1`/`b2`/`a2` `.raw.log` and `.stats.json` under
the laptop's `C:\Users\ramaz\Documents\Codex\2026-10-11\lade\work`.

The final GPU-only 2.2.8 build, with both pacing experiments removed, passed
all 811 ordinary workspace tests, formatting and workspace/all-target Clippy
with warnings denied. Four quiet 20-second loopback streams at 1080p60 and
40 Mbps, with `gpu_compute_conversion=false`, decoded every picture: HEVC
1198/1198, HEVC HDR 1198/1198, PyroWave SDR 1181/1181 and PyroWave HDR
1199/1199. Steady rates were 59.994–60.002 fps, with continuous audio and
zero decode errors. Raw results are under `work/gpu-only-e2e`.

Evidence is in `work/phone-tests/phone-20261012T010118-447124Z`, under
`run-20261012T011159-475248Z/measurement.json` and
`run-20261012T011751-458854Z/measurement.json`.

The native C receiver matrix in
`work/pacing-native/20261012T012146-056459Z/result.json` completed eight
30-second ABBA runs at an 800 Mbps wire target. Mean reported thread cycles
per frame, baseline/burst, were 14.562M/12.769M at 150 Mbps/60 fps and
2.290M/1.597M at 80 Mbps/120 fps. These raw `QueryThreadCycleTime` counts are
not converted to time. `GetThreadTimes` was inconsistent: the two 80 Mbps
burst CPU readings were 12.66% and 0.31% despite similar cycle counts. Both
150 Mbps arms had kernel receive overflows with the unchanged 425,984-byte
buffer. The 80 Mbps runs had no missing packets, but one baseline had 74
duplicates, so the matrix was not entirely clean. The phone gate failed;
no ten-minute soak was run or claimed.

## October 12 resource use (issue 23)

[Issue #23](https://github.com/RamazanKara/Rubylight/issues/23) separates
estimated resource savings from measurements. Release 2.2.7 implements the
following changes without changing the network pacer's waits or capture slot
count:

| Area | Change |
| --- | --- |
| Ping and control workers | Nonblocking media sockets wait for readability with a 250 ms idle bound. Control waits up to 250 ms without peers; connected peers use the next 8 ms feedback deadline or an earlier delayed mouse-release/key-repeat deadline. Input arrival wakes the socket immediately. Control MMCSS registration ends when its last peer leaves. |
| Background work | Empty stall monitoring and inactive telemetry block. Idle housekeeping avoids spawning work or cloning settings. Service supervision waits on stop/session events and child exit, with timed restart and no-session retries. Display leases retain fast recovery checks and use 250 ms idle checks, with immediate teardown wake. |
| Helpers and logging | Helpers dispatch before Tokio starts. The display watcher receives a completion event when the journal becomes empty; a subsequent display change starts a new watcher. Both log channels use 8192 lossy slots instead of the default 128,000. |
| Video transport | `ring`, already present in the lockfile, seals video packets. Other nonce formats keep their existing crypto path. Conventional streams build and send one FEC block at a time, reuse shard allocations, and stop building unsent blocks after loss or cancellation. |
| GPU resources | Cursor uploads retain their resources to a fence instead of blocking the CPU. AMF compute captures are retained through conversion rather than through encoder output. Capture and PyroWave owners forget imported texture references. Copy reuse waits for that texture's readers rather than unrelated conversions. `Device` shares its display metadata through `Arc`. |
| GPU and power policy | The last streaming scope restores the previous process power-throttling and GPU scheduling settings. Nested streams retain the streaming policy until the final scope ends. |
| Graphics conversion | Equal-sized PyroWave 4:4:4 planes use one MRT draw. Their SDR/HDR, scaling, letterboxing and cursor results are compared to the previous three-draw path by the native pixel fixture. |
| Queries and settings | Known AMD-only/explicit-AMF startup probes skip unsupported standard-codec 4:4:4 modes. `/serverinfo` caches only native hardware/route facts for ten seconds; authorization and session fields stay live. mDNS checks addresses every thirty seconds. Effective settings rebuild after config/app/live-HDR changes, with a thirty-second refresh for selected ICC calibration. Cached releases retain consumed fields, and the update panel stops polling while hidden. |

The ping constants imply about eight empty-socket timeout wakeups per second
across both threads instead of about two thousand. That is a code-derived
estimate, not a Windows context-switch or idle-power measurement. The smaller
log queues similarly reduce reserved queue storage; no RSS reduction is claimed
from the constants alone.

The Windows `video_packet_performance` probe compares encrypted and unencrypted
32-, 192- and 576-shard frames, including a reusable-buffer variant. The
pre-change and new encrypted packets have identical counts, lengths and first
frame SHA-256 values. Initial collected-frame medians were 41/353/1071 us before
and 17/223/670 us after; concurrent builds introduce noise, so these are CPU
microbenchmarks, not an end-to-end streaming claim. The reusable variant must be
compared per workload; its largest-frame run did not uniformly beat collected
packetization under that load.

Regression coverage includes legacy media PING, reconnects, delayed input
deadlines, independent AES-GCM parity, lazy FEC blocks and nonce exhaustion,
cache privacy/expiry, AMD probe selection, release metadata, display-watcher
completion, GPU capture ownership and graphics pixel parity. On the RX 7900 XT,
all eight native graphics tests passed, including exact MRT/three-pass pixel
parity with SDR/HDR, scaling, letterboxing and cursors. Capture-fence ownership
and both power/scheduling restoration tests also passed. Workspace Clippy with
warnings denied and the web check/build passed. The local core suite passed 339
tests and the host suite passed 204; two Windows tests require pipe/firewall
access denied by the local sandbox. The Playnite helper integration passed with
a shorter temporary directory (the generated workspace path exceeded Win32's
legacy path limit). These checks do not establish the issue's WPR/VRAM,
saturated-stream or ten-minute network pass criteria.

Remaining experiments from the issue: timer-only or burstier pacing, reducing
the four WGC helper slots to three, persistent codec-probe results keyed by full
driver/AMF identity, and process/GPU resource readings in the timings/API. These
need their own measurements or complete invalidation/measurement support.
NV12/P010 and 4:2:0 graphics fallback targets have unequal sizes and cannot use
the same MRT change; their existing conversion remains. No resource-saving
estimate is presented as a measured latency, picture-quality or recovery gain.

## October 10 PyroWave SDK 1.0 and a high-priority encode queue: shipped

PyroWave moves from `186f0393` (API 0.6) to `502a3b52` (API 1.0,
`libpyrowave-shared-1.dll`). The per-frame bitstream is the one upstream
froze as v1 on 2026-10-03: the quantizer, dequantizer and block-packing
shaders are unchanged, `pyrowave_common.hpp` only renames two enums and adds
an on-disk header, and the decoder only gains an opt-in range scale. So
Vibepollo 2.0 clients and Rubylight Android keep decoding it, and the host
still advertises bitstream `186f0393`. Vibepollo's encoder buffer-pool patch
is dropped because upstream now pools those buffers per frame context; the
4:4:4 sizing and short-block patches still apply. Upstream also fixes an
out-of-bounds read in the RDO analysis shader (efb6230).

New in 1.0 is a global priority for the Vulkan compute queue. The encode
queue now asks for HIGH, or REALTIME when `compute_queue_realtime` is on,
the same policy as the D3D12 copy and conversion queue, and falls back to
the default when the driver refuses. The log line "PyroWave encode queue
priority" records what was granted.

Host A/B on the RX 7900 XT (driver 26.9.2), base `2f53043` (SDK 186f0393,
MEDIUM) against `957ace5` (SDK 1.0, HIGH; the log shows `requested=512
granted=512`), isolated test hosts and the loopback fixture client built
against the old SDK, 20 s per run, three alternating repeats per cell,
loaded runs beside `gpu_load 60 1000 60 200`:

| Cell | Picture age mean / p95 / p99 (ms) | Encode mean / p95 (ms) | Unique fps |
| --- | --- | --- | --- |
| 1968x2184 HDR 4:4:4 120, idle, base | 18.7 / 27.8 / 33.8 | 1.16 / 1.54 | 119.9 |
| same, new | 18.7 / 27.2 / 33.0 | 1.05 / 1.46 | 120.0 |
| 1968x2184 HDR 4:4:4 120, loaded, base | 85.4 / 139.7 / 146.6 | 1.09 / 1.52 | 100.3 |
| same, new | 87.6 / 142.2 / 145.4 | 1.01 / 1.30 | 112.7 |
| 1080p120 SDR, idle, base | 14.8 / 15.9 / 17.9 | 0.66 / 0.81 | 120.0 |
| same, new | 14.7 / 15.7 / 16.8 | 0.65 / 0.80 | 120.0 |
| 1080p120 SDR, loaded, base | 20.2 / 26.6 / 32.2 | 0.72 / 0.90 | 115.8 |
| same, new | 20.6 / 24.8 / 29.2 | 0.70 / 0.88 | 116.3 |

Every run decoded every frame (0 decode errors, 0 replaced frames), so the
old-SDK client reads the 1.0 host's stream. HIGH is no worse end to end:
picture age is within run-to-run noise in every cell, and encoding is 0.1 ms
faster at native size. The loaded native cells fail the e2e gap checks on
both builds: the load also slows the loopback client, which decodes on the
same GPU, so those cells compare the two builds but say little about a real
client. No GPU watchdog dump during the runs.

## October 10 WGC helper textures read in place: on by default

On the service host WGC runs in a helper in the user's session, which
copies each frame into one of its shared textures; the host then copied
that into a texture of its own before the compute conversion. The host now
lends the helper's texture to the conversion directly and hands it back,
after the compute work submitted so far, when the last clone of the image
is dropped (`wgc_helper_zero_copy`, AMD with compute copies only). The
helper got a fourth texture so a held image does not starve it.

Host A/B on one build, 1440p120 AV1 to the loopback test client with
motion_probe, three alternating idle pairs of 30 s: mean picture age 10.07
ms copied against 9.84 ms in place, host mean 2.843 against 2.774 ms,
barcode coverage 1.0 and no decode errors in both. In place, a reconnect, a
client kill and a 2-minute stream (119.6 fps) were clean. A pair beside
`gpu_load 45 1000 0 200` saturated the GPU and both arms fell to about
31 fps, so it could not tell them apart.

## October 10 AV1 entropy-context (CDF) update: no default change

Hidden settings `amd_av1_cdf_update` and `amd_av1_cdf_frame_end` (f546589)
set AMF's Av1CdfUpdate and Av1CdfFrameEndUpdateMode for an offline sweep on
the RX 7900 XT. The driver's defaults are already update on and frame-end
mode 1. Update off cost 5-10% more bytes at the same request with PSNR
0.03-0.66 dB lower and VMAF about unchanged. Frame-end mode 0 was at best
+0.024 VMAF with encode p99 within 0.3 ms either way; mode 2 is rejected
(AMF error 4). VMAF was saturated (98.4-99.8) and AMF undershot the
requested bitrate (35-55 Mb/s actual), so nothing here beats the driver.
The settings stay hidden for experiments; the defaults don't change.

## October 10 per-frame send-wait budget: not built

The idea (triage L7/N2): a batch that finds the video socket full waits up
to 4 ms (`writable` in `net.rs`), so a frame should share one wait budget of
a frame period instead of 4 ms per batch. Counting with the real FEC layout
and batch sizes (packet size 1392, 20% FEC, Wi-Fi pacing at twice the
bitrate, wired at the 800 Mbps ceiling), a frame on a socket that stays full
is already abandoned by send-loss after its first refused batch: one 4 ms
wait for every P-frame from 20 Mbps up and every keyframe from 150 Mbps up,
wired at any bitrate. Only keyframes on a Wi-Fi host at 10-80 Mbps take
2-9 refused batches (8-36 ms); a budget would cut that to about one period,
and only during a host radio outage long enough to fill the 1 MiB socket
buffer (about 100 ms or more). Under congestion, where the wait finds room,
a budget would turn late frames into lost frames and keyframes. Not worth
a host cycle; the draft (543614c, with a `:full` mode for
`BUTTERPOLLO_TEST_SEND_OUTAGE`) was not merged.

## October 10 MMCSS boost on the media threads: not shipped

`Priority::new` registers a thread with MMCSS and then sets
THREAD_PRIORITY_ABOVE_NORMAL, which cancels the boost (14 instead of 18).
The input threads dropped that call in October (`Priority::input`, 3.5 ms
to 17 us wake p50 beside TIME_CRITICAL spinners). Giving capture, encode,
send and audio the same (4b88b6a, research item L2) made the stream worse
with no load. Host ABBA, HEVC 2560x1440 at 120 fps, 50 Mb/s, A = `c7e258a`:
picture-age mean 12.5 (A) against 14.3 ms (B), p95 15.4 / 18.3 against
17.9 / 18.9 ms; unique fps and host mean equal, 0 decode errors. An earlier
no-load round agreed (11.7 against 13.1 ms). Loaded pairs were not run:
TIME_CRITICAL spinners on every core freeze the desktop of an occupied PC.

Likely cause, not measured: every media thread ends its waits in a short
spin (`Timer::until`, and `until_precise` yielding for 600 us), and at the
MMCSS level of DWM's compositor those spins can hold a core the compositor
or the game wants. A boost for these threads needs spin-free waits first.
Dropped; the media threads keep `Priority::new`.

## October 10 HEVC reference invalidation after short losses: not shipped

The HEVC freeze above needs a long loss. H.265 8.3.1 derives a picture's
order count from the previous picture's and reads a step of more than half
the range as going backwards. AMF's range is 16, so after the 11-frame loss
the recovery frame (12 after the last picture) was placed before its
long-term reference, which then resolved to a missing picture. A branch
(`7ff31be`, `58fa344`) kept HEVC long-term references and invalidated only
when the recovery frame was at most 8 frames after the last picture, else a
keyframe. The native fixture passed: HEVC recovered 4-frame losses and took
a keyframe for an 8-frame loss (recovery 9 after), AV1 recovered both.

Host A/B against `02509c0`, HEVC 2560x1440 at 120 fps, 80 Mb/s, 160 Mb/s
cap, no outage: B delivered 103.8 unique pictures/s against 119.6, picture
age mean 14.6 against 8.8 ms (p95 18.4 against 9.5). The repeat B run
stopped after 14 s in a fourth VIDEO_ENGINE_TIMEOUT (0x141,
`WATCHDOG-20261010-1020.dmp`) after `slow encoder call AMF QueryOutput
3000 ms`; the GPU recovered without a reboot. The earlier steady-state pair
with HEVC long-term references (`9438e9c`, set by config) was within noise,
so the cost is not settled, but a steady-state loss and a hang on the first
two runs fail the ship bar. HEVC keeps keyframe recovery; HEVC long-term
references stay off. Not retried: marking long-term references only after a
loss report cannot work, since recovery needs an anchor from before the loss.

## October 10 AMD reference invalidation, AV1 only

Moonlight can recover a lost frame by reference invalidation: the host
encodes the next frame from an older long-term reference instead of sending
a keyframe. AMF offered it only with `amd_ltr_frames` set (default 0).
Fixture: 2560x1440 at 120 fps, 80 Mb/s under a 160 Mb/s pacing cap, an 80 ms
outage every 1.5 s lost after the socket
(`BUTTERPOLLO_TEST_SEND_OUTAGE=1500:80:air`, so only the client notices),
test client with `BUTTERPOLLO_TEST_RFI=1`.

| AV1 | keyframe recovery | invalidation |
| --- | --- | --- |
| Stall mean / p95, set 1 | 106.0 / 106.5 ms | 99.6 / 100.2 ms |
| Stall mean / p95, set 2 | 103.4 / 106.5 ms | 96.4 / 100.0 ms |
| Stall mean / p95, set 3 | 96.1 / 99.6 ms | 91.7 / 92.9 ms |
| Recovery frame | 98-127 KB | 15.5-24.6 KB |
| Steady state (no outage) | picture age 9.35 ms, encode 2.90 ms, 17.8 KB | 9.32 ms, 2.88 ms, 17.8 KB |

Every AV1 frame decoded. HEVC and H.264 did not survive it. After the first
HEVC invalidation the client's d3d11va decoder rejected every later picture
("Error constructing the frame RPS") for 58 s without asking for a keyframe.
The header trace shows why: AMF writes a four-bit POC (it wraps every 16
frames), and after an 11-frame loss the long-term reference it names
resolves to a missing picture. H.264 kept showing pictures up to 2.5 s old
for the rest of each run.

Shipped (`b78f853`, `7c84750`, `204ecdd`): `amd_ltr_frames` defaults to 4 and
applies to AV1 only. H.264 and HEVC answer an invalidation with a keyframe.
After any AMF invalidation recovery a keyframe follows one second later, so
a decoder that cannot follow it is frozen for about a second, not until the
next loss. Moonlight-qt's D3D11VA renderer (Windows, the laptop client)
offers invalidation for HEVC and AV1, not H.264 (`getDecoderCapabilities`
in moonlight-qt master), so a real Windows client streaming AV1 uses this
path, and an HEVC client now gets keyframes.

The first H.264 run of the last set, on the base build (no invalidation
offered, so plain keyframe recovery), ended in a third RX 7900 XT
VIDEO_ENGINE_TIMEOUT (0x141). The stall watch logged the session thread
stuck in its encode call with the GPU query probe answering; the call never
returned. All three timeouts (02:47 and 23:33 on October 9, 07:21 on October
10) came under keyframe-heavy recovery at high bitrate, two of them on base
builds. A call blocked in the kernel driver cannot be ended by the host.
The host's 07:21 session had `LTRFrames=0` and no client invalidation in
the 1.5 s before. Driver 32.0.31041.1004 (2026-08-17) left 24 WATCHDOG dumps
over October 6, 8, 9 and 10 (October 6-8 predate this week's work); the
02:47 and 23:33 dumps share `amdkmdag.sys+0x1d0fb0`. The hang is chronic on
this driver; troubleshooting.md now describes it for users.

## October 10 item 5: the video sender thread, on paced links only

The October 9 sender thread (encode and send overlap on their own threads)
cut p95 by 1.8 ms under a 160 Mb/s pacing cap but added 1.8 ms on the default
800 Mb/s wired path. It now starts only when the pacer rate is within 4x the
stream bitrate (`network_pacing::paced`): a `pacing_max_bitrate_kbps` cap or
a wireless route (2x bitrate). Wired streams keep the inline send path
unchanged. `video_send_thread = false` turns it off. The send-loss recovery
above runs on both paths.

Host A/B, 2026-10-10 (RX 7900 XT, loopback): A = `be942ba`, B = `1984b63`.

| | A | B |
| --- | --- | --- |
| 160 Mb/s cap: picture-age p95, ms | 19.4 | 16.0 |
| 160 Mb/s cap: claim-wait p95, ms | 4.26 | 1.04 |
| Default wired: picture age | equal (inline path in both) | |
| 80 ms outage every 1.5 s: stalls | equal | |

Watch item: on the paced path the recovery keyframe's p95 is 2.2 ms later
(95% CI +0.1 to +4.3 ms). One B run failed "pictures missing"; the host
reads it as fixture variance. Reconnect and client kill
pass, 0 decode errors. Build check: 160 host tests, `clippy -D warnings`
clean. Ships.

Follow-up `0d2ffd2`: on a keyframe request the sender thread skips the delta
frame waiting in its queue (the client discards it; its wire number is left
as a gap, so reference invalidation numbering holds), and a keyframe
replaces a waiting delta frame. Host A/B at the 160 Mb/s cap, A = `c1c8470`:
recovery keyframe p95 65.1 / 64.6 / 65.2 ms (A) against 60.7 / 60.2 / 61.2
ms (B), paired -4.3 ms (CI -4.4 to -4.0, 3 of 3), back to the inline path's
level. Pictures missing at most 1 per request (7 of 60), as designed.
Picture age within noise. Outage-pair stalls 91.6 / 92.9 / 93.0 (A) against
92.1 / 97.2 / 100.5 ms (B), 0 decode errors. 161 host tests, clippy clean.
Ships.

## October 10 Wi-Fi wave 2: a keyframe right after a frame the host could not send

On a host whose own Wi-Fi drops out (the October 9 user report: a Legion Go
host whose socket refused 11-371 video packets at a time, each followed by a
recovery request), the client loses the frame and, as Moonlight does, asks
for a keyframe only after its next complete frame and a round trip. The host
knows at once: Winsock refused the packets.

Change:

- Each frame's send counts refused packets per FEC block
  (`stream_policy::SendLoss`, upper bound when a batch spans blocks). Once a
  block loses more than its parity, the rest of the frame is not sent and the
  next frame is a keyframe (`Session::request_send_loss_recovery`, counted as
  `send_loss_recoveries`, not as a client request). Moonlight, already waiting
  for a keyframe, recovers on that frame and sends no request of its own.
- A frame refused from its first packet (radio out) always forces the next
  keyframe, so the first frame through after the outage is one. A frame that
  got partly in and was then refused (congestion) forces one only if no
  forced keyframe is outstanding within max(4 periods, 40 ms)
  (`SendLossRecovery`); otherwise the client asks, as before.
- Not covered: loss in the air on the client's side (the laptop run's ~80-100
  ms dropout), which the host cannot see.

Test hook for the A/B: `BUTTERPOLLO_TEST_SEND_OUTAGE=every_ms:length_ms`
refuses all video datagrams for `length` of every `every` (from the second
interval on), in both builds. `rust/tests/recovery_gaps.py` reports the
picture stalls in the test client's timing CSV.

Host A/B, 2026-10-10 (RX 7900 XT, idle, loopback): A = `935b514` (test hook
only), B = `c2dbfc5`. HEVC 2560x1440 at 120 fps, 50 Mb/s, strip at the stream
rate, 3 alternating pairs of 30 s with an 80 ms outage every 1.5 s, then 2
pairs with no outage.

| Outage runs | A | B |
| --- | --- | --- |
| Picture stall mean / p95 / max, ms | 97.7 / 101.7 / 101.8 | 88.4 / 92.6 / 93.2 |
| Frames missing per stall | 10.8 | 9.6 |
| Client keyframe requests per run | 17 | 1 |
| Receiver fps | 113.0-113.3 | 113.8-114.4 |

Per-run stall means do not overlap. Without outages both builds pass, host
mean is 3.13-3.14 ms in both, picture-age p95 is 11.4 / 9.1 ms (A) against
9.3 / 9.3 ms (B), and B records no `send_loss_recoveries`. 0 decode errors,
no GPU guard abort, no new dumps. Build check on the host: 153 host tests,
`clippy -D warnings` clean. Not measured: a real Wi-Fi host, a real client,
AV1 and H.264. Ships.

## October 9 research triage: host verification of the five BUILD NOW items

Codex implemented the five BUILD NOW items from the research triage as stacked
commits on `codex/triage-b` (base `74c0d3cf`). A verifier reviewed the code
and measured on the host (RX 7900 XT, evening of October 9, installed host
idle before every run). Only item 1 is on `codex/verified`. The review is in
`butterpollo-release/research/verify-review.md`; it found no blocking code
defect. Runs, scripts and logs are in `research/verify/`.

| # | Item | Commit | Verdict |
| --- | --- | --- | --- |
| 1 | Benchmark integrity gate and per-frame send trace | `3605c124` | ship |
| 2 | AMF ten-bit eligibility, applied-bitrate bookkeeping, HDR metadata test | `91add468` | needs more: the extended HDR test fails on the host |
| 3 | Link-speed lookup off the video threads | `88505c51` | drop: its own step-0 gate failed |
| 4 | Client FEC status telemetry (0x5502) | `71344486` | needs more: no real client report observed |
| 5 | Bounded, ordered sender for H.264/HEVC/AV1 | `735629df`, `66897618` | needs more: no picture-age gain beyond noise |

**What could not be measured.** The isolated e2e fixture streams the primary
monitor, an Odyssey G9 at 5120x1440 and 240 Hz with HDR off. Native
1968x2184 HDR120 needs the virtual display and the SYSTEM batch, which were
out of scope. The proxy for it is HEVC 5120x1440 at 120 fps SDR (1:1 with the
desktop, 7.4 Mpx). No Wi-Fi client, laptop or real Moonlight client was used,
and loopback has no loss. So FEC reports, RFI after loss and link changes
were not exercised.

The fixture: hardware-decoding receiver (`BUTTERPOLLO_TEST_HW_DECODER=d3d11va`),
WGC, `pacing=trace`. Default pacing on loopback is the 800 Mbps wired ceiling
(`link_bps=0`). The client always enables video encryption (`ENCFLG_ALL`), so
every run checked the IV path.

For the sender cells the high-entropy source was a click-through noise window.
It covered the right 34% of the screen above the motion strip and gave HEVC
frames of about 71 packets at 80 Mb/s. The first full-screen version quit on
any user input, and all 20 of its attempts were discarded while the owner
worked. None of the partial-window runs was tainted.

**1: trace cost.** HEVC 5120x1440 at 120 fps, 80 Mb/s, 60 s runs, three
pairs, host mean in ms:

| Build | Log | Host mean per run | Mean |
| --- | --- | --- | --- |
| base | info | 4.983 / 5.047 / 5.056 | 5.029 |
| item 1 | info | 5.114 / 5.044 / 5.047 | 5.068 |
| item 1 | pacing=trace | 5.163 / 5.152 / 5.057 | 5.124 |
| base | pacing=trace | 5.103 / 5.063 / 5.089 | 5.085 |
| item 1 | pacing=trace | 4.973 / 5.035 / 5.067 | 5.025 |

The send fields add nothing measurable on top of the existing claim trace:
traced item 1 is -0.06 ms against traced base, with 11 late intervals
against 21. Untraced item 1 is within the base spread. The 0.056 ms between
info and trace is the cost of `pacing=trace` as a whole, which is
diagnostic only. The gate labelled every run correctly as `matched` (wgc,
5120x1440, 240 Hz, Bgra8). Ship.

**2: AMF.** The default policy snapshots and unit tests pass. The extended
ignored test `hdr10_metadata_and_range_reach_the_bitstream` fails on the
host. A printing copy of it showed the mastering peaks follow every change
(1015, then 600, then 1400 nits) and CLL known, then 0, then known on one
encoder gives 1000/400, then 0/0, then 1200/500, for HEVC and AV1. AMF keeps
the CLL SEI with zeros after a change to 0; a fresh encoder with CLL 0 omits
it. 0/0 means unknown, so the stream is right and the assertion
(absent after known to 0) is too strict. As committed, the test fails the
hardware sweep. It needs the one-line test fix before shipping.

**3: link lookup.** The step-0 timing, 1000 `routed_link` calls to the
laptop's address over wired 2.5 Gb/s: 0.051 ms mean, 0.081 ms p99,
0.561 ms max (Codex: 0.056 / 0.087 / 0.517). That is under the 0.1 ms p99
gate, so the item is dropped. Loopback cannot show a spike either:
`routed_link` returns at once for loopback.

**4: FEC telemetry.** The parser is bounds-checked and has no panic path,
and it changes no decision. The counters stayed at zero in all 73 e2e runs of
builds that carry them, which is expected because loopback has no loss. Host
timing equals base: host mean A against base in the noise cells is 5.13
against 5.13, 5.40 against 5.79, 2.29 against 2.42 and 2.87 against 2.85 ms.
The triage's step 0 is still open: an installed Qt or Android client must be
seen sending 0x5502 to this host, then 10 minutes on Wi-Fi.

**5: sender.** A is `71344486` (inline), B is `66897618`, base is `74c0d3cf`.
Each run is 35 s with alternating order. Cells: (a) HEVC 5120x1440 at
120 fps, 80 Mb/s, default pacing, `--recovery 20`, RFI advertised;
(b) the same with `pacing_max_bitrate_kbps=160000`; (c) H.264 1920x1080 at
60 fps, 20 Mb/s, `--motion-at-rate`; (d) AV1 2560x1440 at 120 fps,
50 Mb/s, `--motion-at-rate`. All with the noise source. The table gives
means over runs; picture age is render to decode, in ms.

| Cell | Build | Runs | Picture age mean / p95 / p99 | Claim wait mean / p95 | Claims held by a send | Fresh claims/s | Claim gap max |
| --- | --- | --- | --- | --- | --- | --- | --- |
| a | base | 3 | 20.4 / 25.4 / 27.6 | 0.45 / 2.48 | - | 120.1 | 13.7 |
| a | A | 7 | 20.6 / 22.6 / 25.1 | 0.59 / 1.57 | 0.1% | 120.0 | 13.2 |
| a | B | 7 | 21.6 / 24.6 / 25.6 | 0.44 / 2.06 | 0.2% | 120.0 | 15.3 |
| b | base | 3 | 25.7 / 30.7 / 33.0 | 1.41 / 4.09 | - | 118.6 | 44.5 |
| b | A | 8 | 24.0 / 28.9 / 30.8 | 1.60 / 5.14 | 58% | 118.9 | 41.0 |
| b | B | 5 | 22.1 / 25.5 / 29.9 | 0.39 / 2.55 | 0.4% | 119.9 | 25.8 |
| c | base | 3 | 14.7 / 18.9 / 25.5 | 0.16 / 0.10 | - | 60.0 | 21.8 |
| c | A | 3 | 15.0 / 16.6 / 17.2 | 0.21 / 0.33 | 0% | 60.0 | 20.1 |
| c | B | 3 | 15.9 / 16.9 / 17.4 | 0.14 / 0.10 | 0% | 60.0 | 20.3 |
| d | base | 3 | 18.0 / 22.1 / 23.6 | 0.89 / 4.27 | - | 120.2 | 12.5 |
| d | A | 7 | 17.9 / 20.9 / 22.7 | 0.57 / 2.73 | 0% | 120.2 | 12.1 |
| d | B | 7 | 16.8 / 20.5 / 21.5 | 0.45 / 2.80 | 0% | 120.1 | 11.5 |

"Claims held by a send" counts fresh claims made within 0.3 ms of the
previous frame's last datagram after the picture had waited more than
0.3 ms. The baseline gate passes. In cell b, 48-64% of A's claims wait
behind the inline send (send occupancy p95 3.9 ms at 160 Mb/s). B removes
that wait: claim-wait p95 drops from 5.1 to 2.5 ms, fresh claims rise from
118.9 to 119.9 per second, and the longest claim gap falls from 41 to 26 ms.

The end-to-end requirement is not met. B's picture-age p95 and p99 in
cell b (21.4-30.3 and 25.0-36.6 per run) overlap A's ranges (26.7-32.6 and
27.8-34.4). A verifier variant of B without items 3 and 4 (local commit
`73181d10`, link lookup kept on the sender thread) scored
24.0 / 28.0 / 30.5 over 3 runs. Picture age with the strip at the stream rate
depends on the capture phase: claim-wait p95 is bimodal, 0.1-1 ms or
5-6 ms, in every build. Cells a, c and d are equal within the spread. In
cell a, B's p95 is 2 ms above A but inside both ranges and the base's.

Keyframes at 160 Mb/s take as long either way: recovery p95 is 59.7 ms (A)
against 59.7 ms (B). Cell b fails the e2e keyframe and gap limits for every
build, including base, because a 5120x1440 keyframe takes about 10 ms on
the wire at that rate.

Correctness checks:

- 0 decode errors in 115 runs.
- Wire frame order is contiguous in every traced run.
- 0 packets dropped.
- The `send queue` trace was present in every B run, about 4,200 rows a
  run at 120 fps. Its maximum was pending 1, sending 1, inventory 2 (1 in
  cells c and d).
- `queue_wait` p95 was 0.04 ms, max 28 ms, the last behind a 160 Mb/s
  keyframe.
- 20/20 keyframe requests were decoded in cells a and b.
- Stop and reconnect: four sessions on one host process (two normal, one
  receiver killed mid-stream, one normal again) passed for A, B and the
  variant. The killed session was freed in 10 s; no panic, no
  `video sender stopped`, no `ERROR`.

The sender is correct and does what it was built to do. It needs a
native 1968x2184 HDR or Wi-Fi measurement that shows a picture-age gain
before it ships. To ship it without items 3 and 4, the cherry-pick needs the
conflict resolution in `73181d10`.

Items 1 and this section reached main as `10c8cea0` and `d68fc79d`.

### Follow-up, October 9-10: item 2 fixed, item 5 settled

**Item 2: ships.** The extended HDR test now passes on the RX 7900 XT for
HEVC and AV1, both ranges, compute on and off, refs 1 and 4, and defaults and
options. It now requires absence of the content light level only on a fresh
encoder's first IDR. After a change it accepts the SEI/OBU at 0/0, which
means unknown, and fails on any stale non-zero level. The scaling test was
renamed to what it covers, and the invalidation check has a comment.

`codex/verified` is main plus `91add468` and the test fix. Its checks:

- the ignored GPU test, with `BUTTERPOLLO_TEST_FFPROBE`
- `fmt --check`
- `test --workspace`: 619 passed, 0 failed, 46 ignored
- `clippy --workspace --all-targets -D warnings`
- 50 Python tests

All passed.

**Item 5: needs more.** It wins on a paced link but regresses the default
wired path. The comparison was paired A/B with the noise source: the
standalone sender variant (`73181d10`) against its exact base (`3605c124`,
the same tree as item 1 on `74c0d3cf`). The runs are 35 s, alternating,
`--recovery 20`. The CIs are 95%, from a bootstrap over pairs.

| Cell (HEVC 5120x1440@120, 80 Mb/s) | Pairs | Picture age p95 diff, ms | p99 diff, ms | Claim-wait p95, base to sender | Fresh claims/s diff | Receiver unique fps diff |
| --- | --- | --- | --- | --- | --- | --- |
| b, pacing capped at 160 Mb/s | 13 | -1.81 [-3.33, -0.26] | -1.38 [-2.41, -0.35] | 3.85 to 1.17 ms | +0.77 [+0.54, +1.05] | +1.48 [+0.43, +2.52] |
| a, default 800 Mb/s pacing | 11 | +1.84 [+0.35, +3.34] | +1.12 [-0.44, +2.62] | 1.61 to 3.02 ms | +0.12 [-0.23, +0.46] | -2.11 [-4.61, -0.31] |

Picture-age p50 and mean are unchanged in both cells, with differences
within ±0.4 ms.

- Cell b: 63% of the base's claims waited behind the inline send, against
  0.5% with the sender. The longest claim gap fell from 39.3 to 29.4 ms.
- Cell a: the sender's p95 is worse beyond noise, and fewer distinct
  pictures reach the receiver. Two of its 11 runs failed e2e gates (gaps,
  repeats), against none for the base.
- Host latency is +0.07 ms (cell b) and +0.03 ms (cell a). It now includes
  the handoff to the sender thread.

So it fails the non-inferiority bar on the owner's default wired path. One
plausible cause, not yet confirmed: the claim gate now counts the frame
being sent (`encoding + sending >= 2`), so on a fast link it can defer a
claim that the inline path would have made right after a 0.5 ms send.

Stability of the sender build:

- 0 decode errors, 0 drops and 0 reorders in every run.
- The queue never exceeded 1 pending and 1 sending.
- 20/20 keyframes were decoded in each cell-a and cell-b run.
- Reconnect and client-kill passed.
- A 10-minute soak at default pacing passed: 72,089 frames all decoded,
  1100/1100 keyframes, `queue_wait` p99 at most 0.06 ms in every minute,
  and host overhead flat. The e2e harness flagged this run only because the
  32 MiB host log had rotated and the startup lines were gone; the stream
  itself was healthy.
- A second 10-minute soak, at 160 Mb/s with 1100 keyframe requests, ended
  at 23:33 with an RX 7900 XT driver timeout. That was
  VIDEO_ENGINE_TIMEOUT_DETECTED (0x141) in `amdkmdag.sys`, the same
  signature as the 02:47 timeout on the unmodified base build (see
  `research/tdr`). The host reported the device loss, waited 30 s and
  ended the stream cleanly.

After the reboot, 12 guarded runs (3 pairs per cell, at most 3 min each,
aborting on any audio stall over 100 ms, device loss, stall or new
watchdog dump) had no abort and no new dump.

The sender should not ship as is. Next step: make it neutral on fast links
before re-measuring cell a. One option is to use it only when pacing is
close to the bitrate, as on Wi-Fi routes or with `pacing_max_bitrate_kbps`;
another is to count only pending, not sending, frames in the claim gate.

## October 9 user report: constant jitter, and recovery requests

Report (a user, relayed by the owner, 2026-10-09 08:54Z): "constant jitter".
Their log (rc.28 host on a Legion Go, Radeon 890M, on Wi-Fi; AYN Odin 2
Portal client; 1080p60 H.264, virtual display at 120 Hz, RTSS limit 60;
Hogwarts Legacy through Steam Big Picture) points at the game's frame rate,
not at the host's pacing. In the 8.5 min game session only 26 of 101
five-second windows reach 58 fps or more; 37 sit at 39.7 fps and 10 near
30 fps, the 120 Hz V-Sync steps, with frames claimed on arrival (claim wait
0.8 ms mean in the 40 fps windows) and encoded in 3.0 ms. A 40 fps game in
a 60 fps stream is shown for one or two refreshes in turn. Their rc.22 log
from October 8 shows the same steps (12 of 28 windows at 40 fps), before
the rc.27 and rc.28 changes. Recovery requests: 8 in the session, each
within 5 s of the host's Wi-Fi socket dropping video packets
(WSAEWOULDBLOCK, 11 to 371 packets at a time), and no reference
invalidations.

Looking for the report turned up a regression that costs a hitch after
each of those drops (from the code and the model below, not yet measured on
the host):

- rc.27 (`88d1decf`) made arrival pacing (the default, and VRR) spend a
  frame of pacing credit on every encode, including an unchanged picture
  encoded again as a keyframe or after a reference invalidation.
- rc.28 (`5a7f7641`) wakes the stream for every recovery request, so on a
  moving picture the unchanged one is encoded again at once instead of the
  request riding the next new frame. Already in rc.27, a request that came
  in while the previous frame was encoding did the same.
- Together: each request from a client losing packets sends an extra
  keyframe of the old picture and takes the next game frame's slot. With a
  source at the stream rate (an RTSS limit at the stream fps, which rc.27
  made work on RTSS 7.3.7) the credit refills only 1% faster than it is
  spent, so the deficit lingers for about a second and frames go out late
  and uneven; at several requests a second it is never repaid.
- Not affected: PyroWave (intra-only, no recovery requests reach pacing) and
  grid pacing.

Model (`recovery_requests_do_not_hold_back_a_moving_picture`, 10 s,
requests at every phase of the frame period):

| Source and requests | Build | Encodes | Frames sent | Game frame wait mean / p95 / p99 / max ms |
| --- | --- | --- | --- | --- |
| 120 fps game at the stream rate, 0.2-0.6 ms jitter, request every 1.5 s | rc.28 | 1206 | 1200 of 1200 | 0.50 / 2.83 / 3.38 / 3.90 |
| same, every 0.5 s | rc.28 | 1208 | 1193 | 1.61 / 5.62 / 6.87 / 8.42 |
| same, every 0.25 s | rc.28 | 1213 | 1182 | 2.78 / 6.60 / 7.52 / 8.93 |
| | fix | 1200 | 1200 | 0.02 / 0.05 / 0.05 / 0.05 |
| 60 fps strip on a 144 Hz display (14/21 ms gaps), 60 fps stream, every 0.25 s | rc.28 | 601 | 581 of 600 | 6.39 / 10.45 / 11.86 / 11.87 |
| | fix | 600 | 600 | 0.02 / 0.04 / 0.04 / 0.04 |

Fix: while the source moves, a recovery request waits for the next new
picture, for two stream periods or 1.5 source frame intervals after the
current one was presented, whichever is longer
(`stream_policy::reencode_at`); a still or slow screen is still encoded
again at once, keeping rc.28's 2.4 ms keyframe there. An unchanged picture
encoded again for a recovery request spends no pacing credit
(`counts_toward_rate`); static repeats still do, so the PyroWave VRR cap
from rc.27 stays.

How it got past the tests: every host A/B ran over loopback without loss,
so no keyframe was requested while the picture moved; the rc.28 wake was
measured with requests only on a still desktop; rc.27's counting change was
A/B'd with PyroWave (no keyframes) and AV1 without loss; the release check
streamed without requests and with its strip at twice the stream rate; the
laptop smoke checks only average fps and host latency. The release check
(`check.ps1`) now also streams `hevc-recovery`: HEVC with the strip at the
stream rate, the minimum frame rate at its default, and 16 keyframe requests
0.5 s apart (`BUTTERPOLLO_TEST_IDR_PROBE_INTERVAL_MS`), with the host tracing
every claim (`RUST_LOG=info,pacing=trace`). It fails when the host encodes an
unchanged picture again, when a request gets no decoded keyframe, or when the
keyframe p95 exceeds three frame periods; one late frame per request is
allowed beyond the usual 1%. The receiver's pictures seen twice or skipped
and picture age are recorded only: with the strip at the stream rate the
fixture repeats and skips 5-28 pictures in 12 s with no request at all
(`e2e.py --motion-at-rate`, the control). `claims_summary.py` summarizes a
traced host log.

Host A/B, 2026-10-09 (RX 7900 XT, idle, extended layout): rc.28 main
`a5884577` against the fix `58953c4f`, isolated extended virtual display at
240 Hz, 1968x2184 HDR 120 fps HEVC at 80 Mb/s, hardware-decoding receiver,
20 keyframe requests per run, no dropped frames in any run.

| Motion probe | Build | Picture age mean / p95 / p99 / max ms | fps | Pictures sent twice | Claim wait p95 ms | Keyframe mean / p95 / max ms |
| --- | --- | --- | --- | --- | --- | --- |
| 120 Hz (stream rate) | rc.28 | 9.67 / 11.87 / 12.69 / 22.19 | 120.58 | 19 | 2.36 | 6.19 / 8.24 / 8.52 |
| | rc.28 | 10.74 / 12.79 / 13.85 / 24.73 | 120.51 | 19 | 2.11 | 6.29 / 7.75 / 8.05 |
| | fix | 10.40 / 11.03 / 11.55 / 15.45 | 120.00 | 0 | 0.013 | 9.08 / 12.31 / 13.10 |
| | fix | 9.35 / 9.95 / 10.37 / 13.99 | 120.00 | 0 | 0.014 | 9.08 / 12.25 / 13.27 |
| 240 Hz | rc.28 | equal within noise | | 8, 10 | | 6.1, 5.8 mean |
| | fix | | | 0, 0 | | 6.1, 6.7 mean |

A keyframe on a moving picture now waits for the next new frame, about
3 ms later at 120 fps. The still 1080p120 desktop keeps the fast keyframe:
2.42 / 2.49 / 2.71 ms (fix) against 2.30 / 2.37 / 2.57 ms (rc.28).

Release-check stream, HEVC 2560x720 at 60 fps, strip at 60 fps, traced:

| Build | Requests | Host claims / unchanged picture encoded again | Receiver seen twice / skipped | Keyframe mean / p95 / max ms | Picture age mean / p95 / p99 ms |
| --- | --- | --- | --- | --- | --- |
| rc.28 | 16 | 711 / 10 | 42 / 40 | 3.37 / 4.77 / 5.45 | 13.27 / 24.18 / 31.44 |
| fix | 16 | 705 / 0 | 15 / 15 | 4.15 / 7.33 / 8.45 | 12.55 / 16.50 / 32.15 |
| rc.28 | none (control) | all new | 28 / 25 | - | 17.50 / 28.35 / 29.80 |
| fix | none (control) | 706 / 0 | 5 / 5 | - | 21.28 / 23.45 / 25.18 |

## October 9 held fixes: host A/B after the reboot

RX 7900 XT, after the 06:27 reboot, rc.27 installed. Each fix was cherry-picked
onto main `235c3fac` and compared with main itself, A/B/A/B: isolated extended
virtual display at 240 Hz, motion probe at 240 Hz, 1968x2184 HDR 120 fps at
80 Mb/s, hardware-decoding receiver, 35 s per run. Picture age mean / p95 /
p99 ms.

| Build | HEVC r1 | HEVC r2 | AV1 r1 | AV1 r2 |
| --- | --- | --- | --- | --- |
| main | 11.58 / 11.99 / 12.23 | 11.21 / 12.03 / 12.29 | 10.90 / 11.57 / 11.91 | 10.92 / 11.55 / 11.94 |
| resume waits for teardown (`0a5a85f5`) | 11.34 / 12.05 / 12.43 | 11.10 / 11.93 / 12.11 | 10.94 / 11.66 / 12.05 | 10.88 / 11.58 / 11.92 |
| no H.264 B-frame property on HEVC (`e8af3a99`) | 11.04 / 11.69 / 12.11 | 11.61 / 12.06 / 12.29 | - | - |

Both neutral (host mean 3.2-3.4 ms HEVC, 3.0 ms AV1 everywhere, 120.0 fps, no
drops) and shipped. The resume fix is also covered by the GPU-free reconnect
matrix (`reconnect_tests.rs`), now with no ignored cases.

Second batch, against main `33d4a11f` plus the two fixes above (October 9
07:25-07:55), same setup:

| Build | HEVC r1 | HEVC r2 | AV1 r1 | AV1 r2 |
| --- | --- | --- | --- | --- |
| main | 11.23 / 11.72 / 12.09 | 11.22 / 11.64 / 11.92 | 10.61 / 11.19 / 11.51 | 11.36 / 11.74 / 11.85 |
| GPU-reset recovery (`c2fa482f`, re-applied) | 10.96 / 11.52 / 11.84 | 11.29 / 11.90 / 12.06 | 10.68 / 11.32 / 11.74 | 11.01 / 11.59 / 11.82 |
| main (second pair) | 11.14 / 11.79 / 12.05 | 10.91 / 11.51 / 11.90 | 10.87 / 11.48 / 11.74 | 10.85 / 11.50 / 11.81 |
| recovery-request wake (`81c8fb3a`, re-applied) | 11.10 / 11.74 / 12.03 | 11.34 / 11.66 / 11.85 | 10.79 / 11.43 / 15.43 | 10.66 / 11.19 / 11.41 |

Both neutral on moving pictures (the single 15.4 ms AV1 p99 did not repeat:
a third wake run gave 10.85 / 11.45 / 11.73). The wake's target, measured with
the receiver's `IDR_PROBE` on a still 1080p120 desktop (10 keyframe requests
per run, request to complete IDR arrival, mean / p95 / max): main 5.80 / 7.50
/ 8.83 and 6.02 / 7.35 / 8.35 ms, with the wake 2.44 / 2.51 / 2.83 and 2.39 /
2.46 / 2.77 ms. A client recovering from loss on a still or slow screen gets
its keyframe about 3.5 ms sooner instead of waiting for the next frame period.
GPU-reset recovery has no host trigger short of a real driver reset; its
fault-injection tests cover it. Both together (the two commits touched the
same capture subscription and were merged by hand): HEVC 11.21 / 11.63 /
11.79 ms, IDR 2.37 / 2.41 / 2.73 ms. Shipped as `aebecdda` and `5a7f7641`.

Batched virtual display apply (`d685d085`, opt-in), same second-client
fixture as on October 8 (WGC HEVC 1080p60, three other clients each creating a
1280x720 display): with the setting on, each creation made 1 whole-topology
apply of 304-514 ms. Picture age on the first stream was not better: p99 /
max 57.5 / 252 and 79.1 / 364 ms with it, 54.1 / 308 and 43.8 / 298 ms
without. The hitch is the topology change itself, not the number of calls, so
the change is not shipped; it stays on the local `codex/hitchfix` branch.

## October 9 AV1 padding at 1968x2184

**Investigation only; no encoder, shader, codec policy or service changes.**
The RX 7900 XT's extra coded area is generated inside AMF/VCN, outside the
host's logical input texture. The host already reuses textures and converts
only the requested picture. There is no demonstrated uninitialised host
padding to fix, and no measured benefit that justifies changing allocation,
alignment mode or AV1 headers. No new size/alignment logic or unit tests were
added. H.264 and HEVC are unchanged.

This review used source, the installed SDK headers and saved October 2
fixtures. The GPU remains unavailable pending reboot: no hardware tests,
capture, encoding, live streams or service operations were run. New evidence
is CPU-only decoding of an existing synthetic D3D11 AV1 fixture, not a fresh host
measurement. Review artifacts and check logs are in
`C:\Users\ramaz\.codex\artifacts\butterpollo-av1pad-20261009`.

### Alignment and the actual padded picture

The SDK used by `windows/build.rs` includes `AMF/components/VideoEncoderAV1.h`
from `BUTTERPOLLO_FFMPEG_ROOT/include`. Its `Av1AlignmentMode` enum is:

| Value | SDK suffix | Meaning for this RX 7900 XT |
|---|---|---|
| 1 | `64X16_ONLY` | Requires width divisible by 64 and height by 16; cannot accept 1968x2184. |
| 2 | `64X16_1080P_CODED_1082` | Same restriction, with a 1920x1080 exception producing 1920x1082. |
| 3 | `NO_RESTRICTIONS` | Current public constructors; accepts the requested input but permits padded output. |
| 4 | `8X2_ONLY` | Smaller alignment exists in the API; the saved RDNA3 experiment still produced the same padded output. |

AMD documents black padding and the older hardware restriction in its
[AV1 guide](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/wiki/AV1-Encoder#av1-specific-api).
The [SDK header](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/8c648005e07d4309033282bfd9947df2c7e76104/amf/public/include/components/VideoEncoderAV1.h)
also exposes `Av1WidthAlignmentFactor` and `Av1HeightAlignmentFactor`.
AMD identifies newer hardware with smaller requirements in
[issue 423](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/issues/423#issuecomment-2692368597);
setting a newer enum does not remove the RX 7900 XT's hardware limitation.
Read back those capabilities after reboot rather than opening AMF now.

This is **not simply width-to-64 and height-to-2 rounding**: 2184 is already
even. FFmpeg's [AMF crop calculation](https://github.com/FFmpeg/FFmpeg/commit/2128c1773956d96a9f6dad06aabd93ee4715b2e3)
uses the reported alignment factors (64x16 fallback for Navi3x), rounds each
dimension up, then replaces an eight-row bottom pad with two rows because of
a hardware special case. Thus the usual 16 extra columns remain, while
2184 would need eight rows to reach 2192 but gets two, reaching 2186.
This explains more than the old guide's specifically named 1080p exception.
It matches all saved SDR/HDR, mode-3/mode-4 cases in
`butterpollo-rust-20260930/day-work-20261002/av1-geometry/geometry.json`:
1920x1080 -> 1920x1082; 1968x2184 -> 1984x2186;
2184x1968 -> 2240x1968. Property readback still reports the requested sizes.

The saved HDR header has `use_128x128_superblock=0`, so its coding
superblocks are 64x64. That is distinct from input alignment and signalled
frame dimensions: 2186 is not a multiple of 64. Both 1968x2184 and
1984x2186 occupy a 31x35 superblock grid, with partial edge superblocks.
There is no additional whole superblock row or column here.

### Who fills it, and what is converted each frame

`windows/src/amf.rs` passes the requested width/height to `FrameSize`, AMF
`Init` and the converters, and calls `SetCrop(0, 0, width, height)` before
submission. That surface crop controls input; it has not produced an output
crop in the saved bitstreams. In `amf/gpu.rs`, the native D3D11/D3D12 surface
wrappers retain the converted texture without a host-side padded copy.

- `compute.rs::Converter::target` allocates **1968x2184 P010** for HDR (NV12
  for SDR), reuses free targets and caps the pool at eight. There is no
  explicit host clear, but `shaders/color.hlsl::yuv420_cs` writes every logical
  luma and chroma sample before submission; AMF waits on the conversion fence.
- `Converter::convert` dispatches `ceil((width/2)/8)` by
  `ceil((height/2)/8)`: **123x137 groups**, each with 8x8 threads, one thread
  per 2x2 block. The `targetSize/2` bounds check rejects the last four thread
  rows before reading or writing pixels. Conversion covers 1968x2184, not
  1984x2186 or the dispatch envelope of 1968x2192.
- The D3D11 fallback in `gpu_color.rs` likewise allocates the requested size
  and draws full luma/chroma viewports at that size. The CPU input path copies
  only logical rows into an AMF host surface. Resource pitch/tile alignment
  is separate from coded image padding; the host does not fill those hidden
  allocation bytes as pixels. Capture `CopyResource` operations copy capture
  textures, not an AV1-sized 1984x2186 image.

The black border is therefore supplied by AMF/VCN, not a host edge-replication
or clear pass. To check actual pixels without the GPU, FFmpeg 9.0.2 decoded
the existing `av1-1968x2184-hdrtrue-align3.obu` with `-hwaccel none`,
`-c:v libdav1d -threads 2` to planar 10-bit raw samples. In all eight frames,
every pixel in the 16 right columns and two bottom rows has Y=3 and U=V=512.
The adjacent visible right-edge luma is 431/615 in the fixture's two halves.
This is stable neutral, below-limited-range black after lossy decoding, not
replicated picture content or evidence of uninitialised host pixels. It does
not establish a universal raw fill value for every driver/input path.
The input SHA-256 is
`7ab9991b6ba3ab7802a108c856183a379ea7f0b2801169b4ce6dd34e16c003d0`;
the sample counts are in `border-samples.json` in the review artifacts.

### Cropping and the client

The saved keyframe's sequence header says
`max_frame_width_minus_1=1983`, `max_frame_height_minus_1=2185`.
Its frame header has `frame_size_override_flag=0`,
`render_and_frame_size_different=0`, and super-resolution is disabled.
Consequently the decoded and render sizes are both 1984x2186. **Render size
belongs to the frame header, not the sequence header, and is not a crop
rectangle.** The [AV1 specification, section 6.8.5](https://github.com/AOMediaCodec/av1-spec/blob/5e04f3f75e73a5898d7616c47c52f032144b8f80/07.bitstream.semantics.md#render-size-semantics)
defines it as an application display hint with no effect on decoding.
FFmpeg's [libdav1d wrapper](https://github.com/FFmpeg/FFmpeg/blob/c766e0de78b0239a36e49e50573e2297a7b500b4/libavcodec/libdav1d.c#L413-L426)
uses it to derive sample aspect ratio while retaining decoded dimensions.
Rewriting it could therefore imply scaling/aspect changes rather than cutting
off the border. The earlier render-size prototype below is not a general
crop fix; changing coded dimensions without re-encoding the tiles is unsafe.
FFmpeg's AMF container crop side data does not travel in this host's raw AV1
GameStream packets (`amf.rs` output -> `host/src/stream.rs` -> `core/src/packet.rs`).

[Moonlight Qt's FFmpeg path](https://github.com/moonlight-stream/moonlight-qt/blob/a57e947d7185fd01384f6973df4df59ae2968d01/app/streaming/video/ffmpeg.cpp#L1923)
has an explicit RDNA3 workaround: compare decoded dimensions with the
original requested dimensions, and crop right/bottom if each excess is
nonnegative and less than 64. Here that removes exactly 16 columns and two
rows with `av_frame_apply_cropping`. Its
[D3D11 renderer](https://github.com/moonlight-stream/moonlight-qt/blob/a57e947d7185fd01384f6973df4df59ae2968d01/app/streaming/video/ffmpeg-renderers/d3d11va.cpp#L887)
uses the resulting frame dimensions for the source rectangle and texture
coordinates. This path should show the original 1968x2184 picture without a
padding bar or squeezing the padded image; ordinary scaling to the client
window remains. Check for the client's cropping log and a border/grid image
after reboot to establish actual client acceptance.

The owner's exact client/version was not supplied, so that conclusion cannot
be applied to every Moonlight platform or fork. The inspected
[Android MediaCodec path](https://github.com/moonlight-stream/moonlight-android/blob/b48494cb96bff23d8886c4775cc4f39a1075495d/app/src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java)
configures the requested dimensions, uses scale-to-fit, and logs decoder
output-format changes; it has no equivalent negotiated-size crop correction.
Correct display there depends on the device decoder/output crop and needs a
real client check. A requested-size overlay alone does not prove cropping.

### Cost and post-reboot host A/B proposal

| Quantity | Value |
|---|---:|
| Visible luma pixels | 1968 x 2184 = 4,298,112 |
| Coded luma pixels | 1984 x 2186 = 4,337,024 |
| Extra per frame | 38,912 = **0.9053%** of visible pixels, not 1.4% |
| Extra at 120 fps | 4,669,440 luma pixels/s |
| Equivalent extra tightly packed P010 storage | 116,736 bytes/frame; 14.0 MB/s for one write at 120 fps |

The storage/bandwidth figures are arithmetic, not measured transfers. There
is **no extra host conversion dispatch or host copy for this coded border**.
A hypothetical full 1984x2186 conversion would use 124x137 groups instead of
123x137. VCN still codes the border and decoders reconstruct it, but constant
black, the unchanged superblock count, internal layouts and possible driver
copies prevent deriving an encode-time or bitrate penalty from pixel count.
AMF's closed implementation may do additional preparation; source inspection
cannot establish whether it clears or copies its internal padding per frame.

Allocating an aligned surface once and clearing padding once would only help
if it removed a measured AMF internal operation. The host already performs
the proposed visible-only conversion and pool reuse. Edge replication would
need updates when the edge changes. A one-time clear must initialize **each
pool surface**, use correct YUV black (limited P010 Y=64<<6, U/V=512<<6;
full-range Y=0), and finish before AMF reads it; clearing zero bytes would
give incorrect chroma. Merely enlarging `targetSize` would scale the desktop
and increase conversion work, so it is not a safe optimisation.

After reboot, first rerun the existing ignored
`amf::tests::native_av1_geometry_and_hdr_are_preserved` fixture with a fresh
`BUTTERPOLLO_TEST_GEOMETRY_DIRECTORY` and explicit executable paths in
`BUTTERPOLLO_TEST_FFMPEG` and `BUTTERPOLLO_TEST_FFPROBE`. It compares both
alignment modes in SDR/HDR and is expected to fail strict decoded dimensions
on this driver. Preserve that failure criterion. Record alignment capability
readback and the actual client's output crop, including a one-pixel border
and square grid to distinguish cropping from squeezing/scaling.

Only if profiling identifies an internal padding/preparation cost, try a
test-build A/B, **not a default or a new flag**:

1. **A:** Current 1968x2184 HDR input, mode 3, 120 fps and the owner's normal
   bitrate/settings/capture path. Save the bitstream and conversion, AMF
   submit-to-output and end-to-end frame-age timings.
2. **B:** AV1-only 1984x2192 (64x16) coded canvas and pool allocation, explicitly
   black-initialized once per surface, with the 1968x2184 picture at the
   top-left. Keep capture, negotiation, conversion coordinates and 123x137
   dispatch unchanged; use the full canvas for AMF FrameSize/Init/input crop
   and mode 1. This requires a confirmed client crop of 16 columns/eight rows.
   It codes **more** pixels than A, so a win is not assumed. Do not substitute
   a larger negotiated desktop, which would change content and scaling.
3. Before implementing B, add CPU unit tests separating visible size, storage
   size and dispatch: 1968x2184 -> 1984x2192 / 123x137;
   1920x1080 -> 1920x1088 / 120x68; already-aligned 1984x2192 stays unchanged.
   Verify all visible edge pixels and both chroma planes, pool reuse, first
   frame initialization and crop on the real decoder in subsequent GPU tests.
4. Alternate A/B/A/B with the same moving scene, refresh, HDR state, bitrate,
   warm-up and sample count. Compare p50/p95/p99 conversion and encode times,
   frame age, stalls/drops, frame bytes and cropped visible-image quality.
   Reject missing edge pixels, bars, scaling changes, colour contamination or
   gains inside run-to-run variance. Do not ship B without a repeatable win.

Validation passed using the supplied `performance-probe/rust-env.ps1`,
`CARGO_TARGET_DIR=D:\bp-build\av1pad-target` and the serialized
`pwsh -NoProfile -File C:\src\cargo-one.ps1` wrapper:

- `fmt --all -- --check`: passed.
- `clippy --workspace --all-targets --locked -j 2 -- -D warnings`: passed.
- `test --workspace --locked -j 2 -- --skip heartbeat_check_finds_the_monitors
  --skip encoders_open_on_the_configured_gpu`: passed; 590 passed, 44 ignored,
  two filtered out, zero failures. Ignored hardware fixtures were not enabled.
- `git diff --check`: passed. Only this findings section changed.

## October 9 Vibepollo fixture failure

Read-only investigation and fixture repair; no host, receiver, GPU workload,
service operation or build was run. Streaming validation must wait for the
reboot described below. Artifacts are under
`C:\Users\ramaz\.codex\artifacts\butterpollo-rust-20260930\bench-rc21`.

**The zero-video failure is a missing WGC helper in the pinned benchmark
bundle.** The `frz-wgc-*` label makes the fixture write `capture = wgc` for
both hosts. Vibepollo needs `vibepollo-baseline-build\tools\sunshine_wgc_capture.exe`,
which is absent; that directory contains only the display helper executable
and CMake files. The October 8 comparison used `capture = ddx`. The current
`sunshine.exe` SHA-256 is
`a539dd0f4d5fdee4537b317efee07e96b201a0c229e050a6af8a18225edb46cf`,
identical to `vp-vibepollo-nat-hevc-a/matched-settings.json`.

The four attempts failed at different stages. Paths in this list are relative
to `frz-wgc-vibepollo-rN`; host logs are under `config/logs/logs`:

- **r1:** `result.log:3-5` and all three `second-*/result.log` files fail with
  `KeyError: 'csrf_token'` in the Rust `interop_second_ab.py`. There is no
  stream launch. `baseline-20261009-014244-363.log:65` is the startup probe
  warning, before this independent login-contract failure.
- **r2/r3:** `result.log:64-75` shows `/launch` still using `timeout=10`.
  In `baseline-20261009-014341-371.log:180-199`, display setup runs from
  01:43:44.714 to `Executing [Desktop]` at 01:43:55.252 (10.538 s).
  `baseline-20261009-014533-494.log:180-200` takes 10.994 s. The timed-out
  primary never starts RTSP; later secondary clients reach capture and log
  the missing WGC executable (r2:259-261, r3:253-255).
- **r4:** the 60-second launch timeout gets past setup. In
  `baseline-20261009-014716-795.log`, lines 180-198 span 01:47:20.589 to
  01:47:34.391; line 195 creates `\\.\DISPLAY13`, line 218 connects the
  client, and line 221 selects that same output. At **01:47:38.557,
  lines 251-253**, `Failed to launch process: ...\tools\sunshine_wgc_capture.exe,
  error: 2` is followed by `WGC IPC helper failed to initialize; requesting
  capture reinit.` This repeats at lines 291-293 and 318-320.
  `result.log:39,44,54` records `TERMINATED error=-100`, no video traffic,
  and `frames=0 decoded_frames=0 audio_packets=1089`. `renderer.log:1-6`
  reaches first present on DISPLAY13, so selecting the wrong display or
  failing to start the renderer does not explain the absence of video.

The r4 first secondary starts at 01:47:44.489 (`second-clients.json`), after
the first missing-helper error. Its `second-0/result.log:6` returns 503 while
the primary disconnects and tears down (host log:321,346-372). Vibepollo's
`src/nvhttp.cpp:6351-6364` admits only one blocking stream mutation at a time;
this response does not diagnose a driver lock. Later secondaries also receive
zero video and hit the same missing helper. The final `motion_probe.exe`
timeouts in the run-level r2/r3/r4 logs are fixture teardown failures after
these earlier errors, not the original streaming failure.

**The logs do not support stopping the installed Butterpollo service.** r4
creates its own display and selects it for capture after the temporary-lease
warning.
More decisively, the successful October 8
`vp-vibepollo-nat-hevc-a/config/logs/logs/baseline-20261008-164312-585.log:66`
has the identical warning; its `result.log:56,67` reports 3,817 decoded frames
and `INTEROPERABILITY PASS`. In the pinned source at
`day-work-20261002/vibepollo-baseline-source`,
`src/platform/windows/virtual_display_sunshine.cpp:8257-8267` can return an
existing physical display without acquiring a temporary probe lease;
`src/main.cpp:839-841` still emits this warning when there is no owned lease.
It then validates encoders with synthetic surfaces. This is not evidence of
exclusive driver ownership by rc.26. Likewise, the unrecognized
`wgc_slot_aligned_publish`, `gpu_compute_conversion` and `prep_cmd` settings
appear in both successful and failed stdout logs (October 8:49-51, r4:48-50).
Omitting `minimum_fps_target = 20` retains Vibepollo's default 20
(`src/config.cpp:969`).

Edited only the external `run-motion-ab.py`, preserving its original bytes
as `run-motion-ab.py.bak` (SHA-256
`3e39523eb6adec8db538e2755292119bda67ccf2c712c589600ae939676eafe7`):

- Pin Vibepollo to its previously working DDX capture, announce it in the
  run log and record `capture: ddx` in matched settings. Butterpollo's WGC
  label selection and configuration remain unchanged.
- For Vibepollo, run the audio tone for `AB_MOTION_SECONDS` and allow helpers
  that duration plus eight seconds to finish. The former fixed 40-second
  tone and eight-second wait could invalidate the 60-second receiver /
  70- or 90-second renderer cases even after capture was repaired.
- For Vibepollo secondaries only, remove the primary's barcode/tone
  requirements and first-frame dump path. They retain decoding/audio checks
  and `AB_NO_CANCEL=1`; three client cycles and the existing delay remain.
  Butterpollo's secondary environment and timing are unchanged.

`interop_vp_ab.py` already contains the separate CSRF-token fetch, 60-second
launch timeout and no-cancel handling; these were retained. Neither it nor
`run-one-ab.ps1`, `batch-ab.ps1`, `run-motion.py` or the installed service was
modified. CPU-only checks passed: Python syntax, PowerShell wrapper parsing,
the backup hash, eight old/new Butterpollo config comparisons, unchanged
Butterpollo secondary commands/environment/timing, Vibepollo capture and
secondary isolation, helper durations, and the 15-column batch row below.
No streaming success or new performance result is claimed before the reboot.

After reboot, with the installed host idle, use this fresh row in the
`RunsFile` supplied to `batch-ab.ps1` through the existing SYSTEM/session
launcher (HEVC SDR 1080p60, 20 Mbps, 120 Hz source, D3D11VA, three secondary
cycles, 60-second receiver and 70-second renderer):

```text
frz-ddx-vibepollo-r5|vibepollo-baseline-build|hevc|1920x1080|60|20000|120|0||d3d11va|3|60|70||vibepollo
```

This is explicitly a DDX baseline. A WGC comparison needs the matching
baseline WGC helper packaged and capture selection restored. Check actual
source refresh and secondary display identities in the new artifacts before
using the result as a matched hotplug comparison: Vibepollo can join another
game client's existing output (`src/remote_session.cpp:263-268`), so
`per_client` and three connections alone do not prove three new displays.

## October 9 AMF settings review

Source audit of `59bcc477` plus the two fixes below, for the RX 7900 XT
(RDNA3, two VCN instances). No encoder workload, quality probe, ignored test
or stream was run, and the installed service was untouched. The GPU still
needs a reboot; the workspace test run encountered an additional non-ignored
GPU-discovery test, documented under verification below.
The target is 1968x2184 HDR 120 fps in HEVC/AV1, plus 1080p60 and 1440p120.
Performance numbers in this section are earlier measurements, not new results.

The build takes its AMF **1.5.2.0** headers from
`C:\Users\ramaz\git\butterpollo-pw-build\build\_deps\ffmpeg-v2026.516.30821\ffmpeg\include\AMF`.
The three encoder headers and `ColorSpace.h` match upstream tag `v1.5.2`
(`eae4a4b7efc35f8b0a3977a0984c0d642efc4e63`) after normalizing line endings.
The tables below use those exact contracts:
[H.264](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/v1.5.2/amf/public/include/components/VideoEncoderVCE.h),
[HEVC](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/v1.5.2/amf/public/include/components/VideoEncoderHEVC.h),
[AV1](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/v1.5.2/amf/public/include/components/VideoEncoderAV1.h),
and [colour definitions](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/v1.5.2/amf/public/include/components/ColorSpace.h).
SDK availability does not prove that this GPU/driver implements a property.

### Findings and scope of the fixes

- **Fixed: nonexistent HEVC B-frame property.** The generic setter prefixed
  `BPicturesPattern`, sending `HevcBPicturesPattern=0`. The HEVC header has
  neither that property nor a B-picture enum. Policy now requests zero B-frames
  only through `BPicturesPattern` and `Av1BPicturesPattern`. The latter really
  exists in this SDK, but is labelled a VCN5 feature, so it can be unsupported
  and redundant on this RDNA3 host. Both remain optional; failures now use the
  policy's normal warning/readback path. Commit `e8af3a99`, with a CPU unit test
  covering all three codecs. No supported HEVC encoding setting changed.
- **Fixed: misleading AV1 frame-skip diagnostics.** `log_effective` queried
  `Av1RateControlSkipFrame`, while the SDK string is
  `Av1RateControlSkipFrameEnable`. The old `?` could not establish lack of driver
  support. Commit `7f0b4b86` corrects the query and tests all three log names
  without constructing an encoder. AV1 frame-skip policy remains untouched;
  its actual support/value must be read after reboot.
- **No wrong usage, preset, rate-control, colour or intra-refresh enum found.**
  In particular AV1 swaps low/ultra-low usage numbers relative to AVC/HEVC,
  and AVC swaps CBR/latency-VBR numbers relative to HEVC/AV1. The code handles
  both. No proven HDR metadata unit bug was found; preserve the AV1 workaround
  described below.
- **Main latency/quality risks are optional tuning.** PA/lookahead, pre-encode,
  slower usage/presets, extra slices/tiles and large recovery frames can cost
  time or bits. CBR, HRD and a smaller VBV/AU cap trade quality against bursts.
  The current defaults and the existing measured H.264 VBAQ exception stay.
- **Known gaps remain visible in this review.** The full-range setters only
  warn on failure; matrix signalling is inferred from the colour profile;
  HDR metadata write success is not a bitstream inspection. The console's
  input-queue help still incorrectly claims zero forces one for VRR. Its SAV
  help mentions only integrated/discrete sharing, although AMD also describes
  multiple VCNs within one GPU. These are recorded, without expanding this
  patch into console or driver-policy changes.

### AMD guidance and the measured baseline

AMD distinguishes interactive game streaming from broadcasting gameplay.
Its game-streaming recommendations include no B-frames, an infinite GOP,
intra refresh, AQ, CBR and a 0.1-second VBV. At 120 fps that VBV is twelve
frame budgets. Butterpollo leaves VBV driver-derived by default; its explicit
buffer settings request 0.5-2 frame budgets. It agrees on no reordering and
on-demand keyframes, but enables intra refresh only when negotiated and uses
latency-constrained VBR. Those are deliberate streaming tradeoffs.
[AMD recommended settings](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/wiki/Recommended-FFmpeg-Encoder-Settings).

The usage preset is applied first, then the explicit settings override it.
Consequently `amd_usage=high_quality` alone still gets the host's speed
preset, latency VBR and PA-off policy unless those are also changed. AMD
identifies ultra-low latency and speed as appropriate to interactive use;
PA/TAQ can override VBAQ, and adaptive mini-GOP can override a B-picture
pattern. Changing usage therefore requires checking effective settings and
the resulting bitstream, not just the dropdown label.
[AMD tuning and priorities](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/wiki/AMF-Encoder-Settings-and-Tuning-in-FFmpeg).

Earlier evidence in **AMF settings per codec: quality at a given bitrate**
below: speed to quality cost about 0.7 ms HEVC, 6.1 ms AV1 and 1.9 ms H.264
mean encode time in the SDR sweep, without consistent VMAF gain; low-latency
usage cost 6-8 ms. CBR spent more of the target budget and enlarged p99 frames
by 30-67% in the SDR clips and 10-42% in HDR. Turning H.264 VBAQ off improved
VMAF by up to 9.42 at 20 Mbps and is already the default. The later **48 HDR
encodes** found no consistent reason to change HEVC/AV1 AQ, balanced preset
or CBR; AQ-off ranged -0.30 to +0.90 VMAF for HEVC and -0.03 to +0.31 for AV1.
Those HDR clips were SDR sources converted to PQ, not native HDR game scenes.

Two VCNs do not establish two-engine acceleration of this session. The
[October 7 split-frame sweep](PERFORMANCE.md#october-7-amf-split-frame-encoding)
found no consistent gain from on/off/untouched across 720 runs, including
sizes up to 7680x2160; readback was already on. Treat a split as unproven until
timing or engine activity shows it. AMD's SAV description covers both
multi-VCN GPUs and multi-device systems; SAV and the per-codec split-frame
property are separate controls.
[AMD SAV primer](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/wiki/Smart-Access-Video-Primer).

### Complete encoder-property inventory

Sources: `core/src/encoder_policy.rs::{amf,amf_rate_control,amf_split_frame}`,
`windows/src/amf.rs::{create,configure_ltr,prepare_surface,set_bitrate,write_hdr_metadata}`
and `web/src/lib/schema/video.ts`. Names below are literal AMF strings, not
the SDK macro identifiers. Unless noted, properties are set before `Init`.
`auto` omits a write; **unset** below means policy does not write it, even if
`AMF encoder settings` reads it. Policy writes are read back: required failures
abort creation, optional failures warn. Direct `property` calls generally
check only SetProperty success, with the exceptions specified below.

| Setting | H.264 | HEVC | AV1 | Value, SDK comparison and verdict |
|---|---|---|---|---|
| Usage | `Usage` | `HevcUsage` | `Av1Usage` | Default ultra-low = **1 / 1 / 2**. Transcoding 0; low-latency **2 / 2 / 1**; webcam 3; high-quality 4; low-latency-high-quality 5. Correct, required unless auto. Other usages may buffer frames. |
| Quality | `QualityPreset` | `HevcQualityPreset` | `Av1QualityPreset` | Speed **1 / 10 / 100**; balanced **0 / 5 / 70**; quality **2 / 0 / 30**. Correct; default speed, required unless auto. SDK also has high-quality presets, which the console does not offer; no new option proposed. |
| Rate control | `RateControlMethod` | `HevcRateControlMethod` | `Av1RateControlMethod` | Default latency VBR **3 / 1 / 1**; CBR **1 / 3 / 3**; CQP 0; peak VBR 2; QVBR 4; HQVBR 5; HQCBR 6. Correct. Explicit choice is required; default rejection warns. HDR demotes modes 4-6 to peak VBR. CQP ignores the stream bitrate budget. |
| QVBR target | `QvbrQualityLevel` | `HevcQvbrQualityLevel` | `Av1QvbrQualityLevel` | Only QVBR: explicit 1-51, zero leaves default (SDK 23). This is not AV1's 1-255 QIndex. Names/range correct. The headers do not substantiate the console's low/high quality endpoint labels. |
| Adaptive quantization | `EnableVBAQ` | `HevcEnableVBAQ` | `Av1AQMode` | Default false / true / **1 (CAQ)**; AV1 0 = none. Disabled for explicit CQP; auto omitted. Correct codec distinction, though the common console label says VBAQ for AV1 too. AQ may redistribute distortion rather than improve PSNR. |
| Pre-analysis | `EnablePreAnalysis` | `HevcEnablePreAnalysis` | `Av1EnablePreAnalysis` | False by default and for HDR; true on SDR when requested or RC >=4. Required when true. Correct names. The AVC/HEVC header comment saying peak VBR only is stale relative to AMD's documented PA-dependent quality RC modes. |
| PA lookahead | `PALookAheadBufferDepth` | `PALookAheadBufferDepth` | `PALookAheadBufferDepth` | Shared name, **1** only when PA is enabled, required. Adds analysis/buffering risk. No codec prefix belongs here. |
| HRD | `EnforceHRD` | `HevcEnforceHRD` | `Av1EnforceHRD` | False by default, configurable bool; explicit setting required. Correct. Off permits looser buffering; on may raise QP. It is not a network packet pacer. |
| RC frame skipping | `RateControlSkipFrameEnable` | `HevcRateControlSkipFrameEnable` | `Av1RateControlSkipFrameEnable` | Writes false optionally for AVC/HEVC. AV1 **unset**, now logged with the correct SDK name. Confirm actual value/support; do not infer it from the old log. |
| Input queue | `InputQueueSize` | `HevcInputQueueSize` | `Av1InputQueueSize` | `amd_input_queue_size=0` writes nothing for **all** clients; positive requests are clamped to 1-32 and required. SDK default 16 is capacity, not a mandatory 16-frame delay. The VRR claim in console help is stale. |
| Query timeout | `QueryTimeout` | `HevcQueryTimeout` | `Av1QueryTimeout` | **1 ms**, optional direct write; SDK default 0 = no wait. Correct units. QueryOutput may return earlier on output; this does not deliberately delay every frame by 1 ms. A pending query can still occupy the capture/encode thread until timeout. |
| Internal latency | `LowLatencyInternal` | `LowLatencyInternal` | — | Shared **unprefixed** bool for AVC/HEVC, explicit only, required. Correct exception to prefixing. AVC also selects POC mode 2. Under ultra-low usage the driver commonly already enables it; forcing on can be redundant. |
| AV1 latency | — | — | `Av1EncodingLatencyMode` | Auto omitted; none 0, power-saving 1, realtime 2, lowest 3. Correct. SDK targets respectively no deadline, 1/fps, 1/(2*fps), fastest possible; these are effort/power modes, not guaranteed timing. |
| SmartAccess Video | `EnableEncoderSmartAccessVideo` | `HevcEnableEncoderSmartAccessVideo` | `Av1EnableEncoderSmartAccessVideo` | Auto omitted; explicit bool, required only when enabling. Guard forces SAV off (required) when AVC/HEVC **explicitly** requests internal low latency. It does not inspect the usage-derived effective latency value; keep this driver-interaction question separate from a proven reset cause. |
| Split frame | — | `HevcMultiHwInstanceEncode` | `Av1MultiHwInstanceEncode` | Optional bool after other policy: only if `HevcNumOfHwInstances` / `Av1CapNumOfHwInstances` >1; auto asks only if not already on. Correct names; accepted hint is not proof of a split. On this host normally redundant. |
| High motion | `HighMotionQualityBoostEnable` | `HevcHighMotionQualityBoostEnable` | `Av1HighMotionQualityBoost` | Explicit bool only, optional. Correct AV1 exception. Existing sweep cost 0.2-1.1 ms without consistent quality gain. |
| Profile | `Profile` | `HevcProfile` | — | AVC **100 (High)** always; HEVC **2 (Main10)** for ten-bit, otherwise driver Main. AV1 profile left to driver (Main =1). Correct; AVC ten-bit/HDR and all AMF 4:4:4 are rejected. No level/tier override is made. |
| AVC entropy coding | `CABACEnable` | — | — | Auto omitted; CABAC 1, CAVLC 2. Correct enum despite the Enable name; do not replace with a bool. High profile's automatic coding is CABAC in the SDK. |
| Screen-content tools | — | — | `Av1ScreenContentTools` | Explicit bool only, required. SDK defaults true and gates palette/integer-MV tools; turning this on alone need not change anything. `Av1PaletteMode` and `Av1ForceIntegerMv` are not written. |
| Slices/tiles | `SlicesPerFrame` | `HevcSlicesPerFrame` | `Av1NumTilesPerFrame` | AVC/HEVC write negotiated count only if >1. AV1 explicit 1/2/4, or negotiated slices capped at 4 if >1; zero/one automatic leaves driver alone. Correct: tiles are a suggestion, and readback accepts any positive result. Extra partitions may cost coding efficiency; no slice/tile output mode is enabled. |
| References | `MaxNumRefFrames` | `HevcMaxNumRefFrames` | `Av1MaxNumRefFrames` | Negotiated positive count is required; zero leaves driver default. AVC intra refresh requires at least two; a one-reference client falls back to IDR. Correct budget handling; more slots are not evidence that the encoder actually uses extra predictors. |
| B-frame pattern | `BPicturesPattern` | **No property** | `Av1BPicturesPattern` | Optional zero for AVC/AV1 only after this fix. AV1 property is VCN5-only per header and may be absent here; HEVC had an invalid prefixed write. No B-reference, maximum-B, or adaptive-mini-GOP setting is written. |
| Size | `FrameSize` | `HevcFrameSize` | `Av1FrameSize` | Negotiated width/height as AMFSize, required. Correct. |
| Rate | `FrameRate` | `HevcFrameRate` | `Av1FrameRate` | `fps_millihz()/1000` as AMFRate, required; preserves fractional refresh. Correct. |
| Target | `TargetBitrate` | `HevcTargetBitrate` | `Av1TargetBitrate` | Requested kbps *1000 bits/s, required initially; updated dynamically. Correct units. Failed live update warns and retains driver state. |
| Peak | `PeakBitrate` | `HevcPeakBitrate` | `Av1PeakBitrate` | Default unset; configured ratio 1-2 * target bits/s, required if requested. Not an individual-frame limit; its relevance depends on RC mode. |
| VBV | `VBVBufferSize` | `HevcVBVBufferSize` | `Av1VBVBufferSize` | Default unset; configured 0.5-2 * bits/frame, required if requested. Written after frame rate and target; **bits**, not bytes or milliseconds. Correct. Small buffers can hurt detail/scene changes. |
| Maximum AU/frame | `MaxAUSize` | `HevcMaxAUSize` | `Av1MaxCompressedFrameSize` | Default unset; configured 1-8 * bits/frame, required if requested. Correct names and bit units. Zero config means omit, not actively clear. HEVC header's default-60 comment disagrees with API guide's zero; read driver state instead of assuming either. |
| GOP/IDR | `IDRPeriod` | `HevcGOPSize` | `Av1GOPSize` | **0**, AVC failure ignored, HEVC/AV1 required. No scheduled recovery IDRs; request on surfaces. `HevcGOPSPerIDR`, AVC `IntraPeriod`, `Av1IntraPeriod` and insertion cadence stay unwritten. Finite-GOP stalls are a retained host workaround, not an AMD guarantee for every driver. |
| Intra refresh | `IntraRefreshMBsNumberPerSlot` | `HevcIntraRefreshCTBsNumberPerSlot` | `Av1IntraRefreshMode`, `Av1IntraRefreshNumOfStripes` | Only when negotiated: AVC 16x16 MBs, HEVC 64x64 CTBs; N=ceil(w/block)*ceil(h/block), request ceil(N/clamp(N,1,299)) per slot. AV1 **continuous=2**, **300 stripes**. Correct enums/block units; approximately 300 pictures means ~2.5 s at 120 fps or 5 s at 60, not immediate full recovery. Small pictures/granularity change the AVC/HEVC duration. |
| AV1 alignment | — | — | `Av1AlignmentMode` | **3 (no restrictions)** on public constructors, required. Correct enum, but this driver still pads 1968x2184 to 1984x2186 without render-size signalling; already measured below. Changing alignment is not a proven fix. |

LTR uses additional component and surface properties; all names and the
reset-unused enum agree with the SDK. `amd_ltr_frames=0` leaves them alone.
A positive request (maximum four) is limited to negotiated references minus
one rolling reference, to property limits and, for AV1, `Av1CapMaxNumLTRFrames`.
Intra refresh disables LTR. Failed setup restores the maximum to zero and
falls back to IDRs. This is loss recovery, not a free steady-state quality gain.

| Purpose | H.264 | HEVC | AV1 | Value |
|---|---|---|---|---|
| LTR capacity | `MaxOfLTRFrames` | `HevcMaxOfLTRFrames` | `Av1MaxNumLTRFrames` | Limited positive count before Init, read back. |
| LTR lifetime | `LTRMode` | `HevcLTRMode` | `Av1LTRMode` | **0 = reset unused**; `core/src/ltr.rs` drops other anchors when recovering, matching the driver rule. |
| Mark surface | `MarkCurrentWithLTRIndex` | `HevcMarkCurrentWithLTRIndex` | `Av1MarkCurrentWithLTRIndex` | Slot index on keyframe/selected every-fourth frames when LTR is active. |
| Recover surface | `ForceLTRReferenceBitfield` | `HevcForceLTRReferenceBitfield` | `Av1ForceLTRReferenceBitfield` | **1 << slot**, not the slot number. Correct. Surface failure disables LTR and requests IDR. |
| Force recovery frame | `ForcePictureType` | `HevcForcePictureType` | `Av1ForceFrameType` | **2 = IDR / 2 = IDR / 1 = KEY** on requested keyframes. Correct. |
| Repeat headers | `InsertSPS`, `InsertPPS` | `HevcInsertHeader` | `Av1ForceInsertSequenceHeader` | True on those keyframes so a reset decoder can start independently. Correct. |

Not written: pre-encode (`RateControlPreanalysisEnable`,
`HevcRateControlPreAnalysisEnable`, `Av1RateControlPreEncode`), filler
(`FillerDataEnable`, `HevcFillerDataEnable`, `Av1FillerData`), and initial
fullness (`InitialVBVBufferFullness`, `HevcInitialVBVBufferFullness`,
`Av1InitialVBVBufferFullness`; SDK scale 0-64). The first two groups are logged
only. PA-off does **not** explicitly switch pre-encode off. Filler can consume
wire budget without picture detail, particularly under a CBR/usage change;
confirm effective values before attributing an actual-bitrate gain to quality.

QP limits are also **unset**, not clamped by the host: AVC `MinQP`/`MaxQP`
and `QPI`/`QPP`/`QPB` (0-51); HEVC `HevcMinQP_I`, `HevcMaxQP_I`,
`HevcMinQP_P`, `HevcMaxQP_P`, `HevcQP_I`, `HevcQP_P` (QP scale 0-51);
AV1 `Av1MinQIndex_Intra`, `Av1MaxQIndex_Intra`, `Av1MinQIndex_Inter`,
`Av1MaxQIndex_Inter`, `Av1MinQIndex_Inter_B`, `Av1MaxQIndex_Inter_B`,
`Av1QIndex_Intra`, `Av1QIndex_Inter`, `Av1QIndex_Inter_B` (SDK 1-255).
Do not reuse an AVC QP number as an AV1 QIndex. These and colour/GOP details
are not all in the settings log; an omitted field is not evidence of a default.

Live bitrate changes also scale positive read-back peak/VBV/AU values by the
new/old bitrate ratio, including driver-derived values, clamped to property
limits. Partial update failure leaves mixed driver state and a warning.
Do rate-control comparisons on fresh sessions at a fixed bitrate.

The session admits two pending pictures; AMF's native ownership limit is
eight. Neither equals the driver's input-queue capacity. `poll` queries only
with work in flight and returns one result promptly. AMD recommends separate
submission/output threads for overlap; this host uses a combined capture/encode
loop with separate sending. A 1 ms query can therefore affect claim timing,
but a threading rewrite is outside this review.
[AMD asynchronous application guide](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/wiki/Guide-for-Video-CODEC-Encoder-App-Developers#53-amf-asynchronous-and-synchronous-model).

PA remains a low-priority experiment. AMD documents NV12 analysis and warns
that PA adaptive quantization can supersede VBAQ. The code gates PA and quality
RC on `hdr`, not `ten_bit()`, so optional **ten-bit SDR** can still request PA
with P010; this needs a format-support check before using that combination.
Auto-LTR/TAQ/adaptive mini-GOP are not explicitly controlled by this host.
Keep PA off for the HDR baseline; no broader policy change is justified here.
[AMD PA guide](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/doc/AMF_Video_PreAnalysis_API.md).
The [HEVC API guide's rate-control section](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/doc/AMF_Video_Encode_HEVC_API.md)
explicitly requires PA for QVBR/HQVBR/HQCBR, explaining the discrepancy with
the older peak-VBR-only header comment; the same guide specifies MaxAUSize
in bits with default zero.

### HDR and colour from capture to Moonlight

`Negotiated::color_matrix()` forces BT.2020 for HDR; `ten_bit()` selects P010
and `full_range()` takes bit zero of negotiated `csc_mode`. The D3D11 and
D3D12 paths use the same `gpu_color::constants` and `shaders/color.hlsl`:
scRGB FP16 (Rec.709 primaries, 1.0 = 80 nits) is converted to Rec.2020 and
absolute ST.2084 PQ before 4:2:0 subsampling. An already-PQ `Rgba10Pq` source
is not PQ-encoded twice. Limited P010 uses Y 64-940 and nominal chroma
64-960; full range uses 0-1023, with neutral chroma 512, stored in the high
ten bits. CPU fallback in `encoder.rs::Convert`/`color.rs` uses the matching
matrix/range and PQ conversion. Input AMF colour properties describe these
**already-converted NV12/P010 surfaces**, not the original scRGB texture.

| Property purpose | H.264 | HEVC | AV1 | Write and assessment |
|---|---|---|---|---|
| Bit depth | No ten-bit write; rejected | `HevcColorBitDepth` | `Av1ColorBitDepth` | 10 for HDR or ten-bit SDR; otherwise SDK default 8. Correct; HEVC also sets Main10. |
| Input profile | `InColorProfile` | `HevcInColorProfile` | `Av1InputColorProfile` | AMF profiles limited 601/709/2020 = **0/1/2**, full = **3/7/8**. Correct. Optional input description is largely redundant with our own conversion. |
| Output profile | `OutColorProfile` | `HevcOutColorProfile` | `Av1OutputColorProfile` | Same values; HDR uses 2 or 8. Correct names, including AV1's Input/Output rather than In/Out. Failed output writes abort HDR creation; SDR warns. |
| Input transfer | `InColorTransferChar` | `HevcInColorTransferChar` | `Av1InputColorTransferChar` | HDR **16 (PQ)**; SDR 601/709/2020 = **6/1/14**. Input failure only debug-logged. |
| Output transfer | `OutColorTransferChar` | `HevcOutColorTransferChar` | `Av1OutputColorTransferChar` | Same values, correct per `ColorSpace.h`; output failure handled as above. |
| Input primaries | `InColorPrimaries` | `HevcInColorPrimaries` | `Av1InputColorPrimaries` | 601/709/2020 = **6/1/9**; HDR always 9. Correct; input optional. |
| Output primaries | `OutColorPrimaries` | `HevcOutColorPrimaries` | `Av1OutputColorPrimaries` | Same values; HDR BT.2020. Correct, required for HDR. |
| Input range | `InputFullRangeColor` | `HevcInputFullRangeColor` | `Av1InputFullRangeColor` | Negotiated bool. Correct names/type; failures tolerated because some drivers reject the input hint. |
| Output range | `FullRangeColor` | `HevcNominalRange` | `Av1NominalRange` | Same bool. These literal names remain correct even though old macro aliases are deprecated. HEVC explicitly permits bool (legacy integer 0/1 also works); no enum/type fix needed. **Failure is only a warning for full range**, so verify actual bitstream range. |
| Matrix coefficient | `InMatrixCoeff`, `OutMatrixCoeff` | `HevcInMatrixCoeff`, `HevcOutMatrixCoeff` | `Av1InMatrixCoeff`, `Av1OutMatrixCoeff` | **Unset**. The driver derives matrix signalling from profile; expected 601/709/2020-NCL = **6/1/9**, distinct from AMF profile numbers. The existing hardware test asserts BT.2020-NCL; no unconditional new property required without driver testing. |
| Static HDR payload | None sent | `HevcInHDRMetadata` | `Av1InHDRMetadata` | AMFBuffer interface containing AMFHDRMetadata, after Init and before first input; updated when changed. AV1 still uses **InHDRMetadata**, not InputHDRMetadata. HEVC's OutHDRMetadata is commented out in the SDK. Correct names. |

`capture.rs::Device::hdr_metadata()` reads the selected output's DXGI
maximum/minimum/full-frame luminance, with bounded finite conversion in
`core/src/hdr.rs`. Primaries are synthesized as Rec.2020 and D65, not copied
from DXGI panel primaries or a game's mastering metadata. Fallback is 1000
nits peak, 0.0001 nit minimum, unknown full-frame luminance. The live path
leaves **MaxCLL and MaxFALL at zero (unknown)**; the quality probe and tests
can supply nonzero values. Display full-frame luminance is not content MaxFALL
and is correctly kept separate. This is valid synthetic desktop metadata,
not preservation of original HDR10/HDR10+ game metadata.

The same `Metadata` value is stored in the session and sent to the encoder
before the first frame (`stream.rs` startup). It is refreshed once per second;
encoder recreation sets `metadata_due` to now, so the first recovery frame
also receives metadata. `set_hdr_metadata` skips SDR/AVC and duplicate values.
On failure it warns and caches the attempted value rather than retrying it
every frame; control metadata can therefore be correct while bitstream
metadata is absent. Even a successful SetProperty/log line does not prove
that every subsequent keyframe carries the metadata. The AV1 header labels
the input HDR property static, while this implementation uses it after Init;
retain the known driver path and check an independent keyframe after updates
on any new runtime before claiming portable dynamic support.

Metadata units are intentionally codec-specific:

| Field | Internal/control value | HEVC AMFHDRMetadata / SEI | AV1 AMFHDRMetadata / metadata OBU |
|---|---|---|---|
| RGB primaries, D65 white | Integer x/y in 1/50000 | Unchanged | floor(value *65536/50000), capped at 65535: 0.16 fixed point |
| Mastering peak | Whole nits (`u16`) | nits *10000 (`u32`) | nits *256 (`u32`): 24.8 fixed point |
| Mastering minimum | 1/10000 nit (`u16`) | Unchanged in 1/10000 nit | floor(value *16384/10000): 18.14 fixed point |
| MaxCLL / MaxFALL | Whole nits, zero unknown | Unchanged | Unchanged |
| Display full-frame peak | Whole nits, zero unknown | No AMFHDRMetadata member; control only | Control only |

The shared AMF header describes HEVC units for AMFHDRMetadata. The retained
AV1 implementation documents that this driver copies those fields straight
into the OBU, so the host pre-scales them to the
[AV1 mastering-display syntax](https://github.com/AOMediaCodec/av1-spec/blob/master/07.bitstream.semantics.md#metadata-high-dynamic-range-mastering-display-color-volume-semantics).
For 1015 nits, minimum 0.005 nit and red x=0.708, the existing ignored test
expects HEVC **10150000/10000**, **50/10000**, **35400/50000**; AV1
**259840/256**, **81/16384**, **46399/65536**. Using HEVC's peak scale on AV1
would signal 39,062.5 nits for a 1000-nit source. The small AV1 truncation is
quantization, not a factor-of-10,000 bug. Hardware tests were not rerun here.

HEVC carries mastering-display and, when known, content-light metadata in
prefix SEI. AV1 carries HDR_MDCV (type 2) and HDR_CLL (type 1) metadata OBUs.
`amf.rs::poll` copies these encoder bytes unchanged for HEVC/AV1; the host
passes them to `VideoPacketizer::encode_recovery`, which frames/FECs/encrypts
the payload without filtering SEI/OBUs. H.264 is SDR-only in this backend,
with colour signalling in its SPS VUI and no HDR buffer.

The retained Moonlight fixture (`performance-probe/moonlight-vrr-common`,
commit `d6a11bc685b41037b352a96f29d08276fe5359ba`) explains the apparent gap
at the receiver: `VideoDepacketizer.c` strips H.264/HEVC leading AUD/prefix-SEI
NALs, including HEVC NAL type 39, before delivering decode units. It does not
parse the AV1 bitstream that way. Thus a Moonlight HEVC decode-unit dump may
lose the metadata that was present on the wire; a dump is not sufficient to
accuse the host of omitting SEI. The client's renderer/decoder can use the
separate control metadata; actual tone mapping is client-specific.

`stream.rs` sends encrypted reliable ENet control type **0x010e** once the
encoder is configured with the display's metadata, and again whenever the
27-byte metadata payload changes. Sunshine also sends it once. It used to go
out as soon as an output was selected, with placeholder metadata, then again
with the real values; Moonlight for Xbox sets the TV's HDMI mode on every
message, so it switched twice (issue #11).
`Metadata::wire` and Moonlight `ControlStream.c` agree exactly:

| Payload offsets | Field | Representation |
|---|---|---|
| 0 | HDR enabled | 0 or 1 from negotiated `config.hdr`, including 0 for SDR |
| 1-12 | R, G, B x/y | Six little-endian u16, normalized to 50000 |
| 13-16 | White x/y | Two little-endian u16, normalized to 50000 |
| 17-18 | Mastering peak | Little-endian u16, whole nits |
| 19-20 | Mastering minimum | Little-endian u16, 1/10000 nit |
| 21-24 | MaxCLL, MaxFALL | Two little-endian u16, whole nits |
| 25-26 | Display full-frame peak | Little-endian u16, whole nits |

Moonlight saves those fields, updates HDR state and invokes `setHdrMode`;
`LiGetHdrMetadata` exposes the copy. Decode units get `hdrActive` and Rec.2020
from that state (SDR uses negotiated colour space); the payload does not
separately carry transfer/matrix/full-range flags. Bitstream signalling and
negotiation still matter. `tests/moonlight_client.c` checks HDR notifications,
metadata validity and decoded ten-bit/PQ/BT.2020 frames. The existing ignored
`hdr10_metadata_and_range_reach_the_bitstream` checks encoder output directly,
both codecs, both ranges, D3D11/D3D12 and independently decodable keyframes
0 and 4, including nonzero/unknown CLL. It is the post-reboot validation to
use, not a test to execute while the GPU is down.

### Ranked host A/B plan, after reboot

Keep ultra-low usage, speed, latency VBR, PA off, the existing per-codec AQ
default, automatic queue/latency/SAV/split-frame and negotiated references as
the control. Use separate fresh sessions; change one setting at a time.
These rankings are expected opportunities, not claims of measured gains:

| Rank | Concrete A/B values | Expected benefit and required observations |
|---|---|---|
| 1 | `amd_max_frame_size=0,4,2` (1 only if the others help) | Best candidate for recovery/scene-change **picture-age tails on a constrained link**. In amf_quality compare IDR bytes, p99 P-frame bytes, actual bitrate and detail loss; in streaming request recovery and measure sender/FEC time, picture-age p95/p99, decode errors and held pictures. A cap may reduce quality or be ignored; readback alone is insufficient. |
| 2 | `amd_ltr_frames=0,1,2`, only with a client reference budget allowing it, intra refresh off | Potentially large loss-recovery benefit, little expected idle gain. Probe steady-state quality/bytes/encode cost first; then controlled packet loss and repeat recovery on a separate client. Measure recovery-frame bytes, time to clean picture and picture-age p99. One-reference clients cannot test LTR; amf_quality does not simulate loss. |
| 3 | HEVC/AV1 `amd_vbaq=enabled,disabled`; H.264 retain disabled as control | Cheap quality opportunity on **native HDR** gradients, particles, foliage and UI. Earlier synthetic HDR changes were small/mixed, max +0.90/+0.31 VMAF for HEVC/AV1. Compare PSNR/SSIM-style plane metrics, visual temporal stability and actual bytes, with no regression in encode p99 or picture age. Do not assume AV1 CAQ is identical to AVC VBAQ. |
| 4 | First `amd_rc=vbr_latency,vbr_peak,cbr`; then, separately with the selected RC, `amd_peak_bitrate_ratio=0,1,1.5`, `amd_vbv_buffer_frames=0,2,1`, `amd_enforce_hrd=false,true` | Better use of bitrate or smaller bursts, with a quality tradeoff. Existing CBR gains mostly came from spending more bits and produced larger frames. Measure actual bitrate, scene-cut/IDR sizes, PSNR/SSIM/VMAF, encode p99 and on-wire picture-age tails; compare both equal requested and equal actual bitrate. Do not switch all four knobs together. |
| 5 | AV1 `amd_av1_tiles=0,1,2,4`; separately `amd_av1_screen_content=auto,disabled,enabled` | Possible coding-efficiency or client decode improvement, especially if automatic tiles exceed one. Inspect effective tile count and screen-tool use; quality-probe bits/text edges and separate-client decode time/picture age. Extra tiles do not make this full-frame sender transmit early and did not unlock split-frame gain in earlier sweeps. AVC/HEVC slice 1/2/4 requires a negotiated test-client change; no host config knob exists. |
| 6 | `amd_input_queue_size=0,4,2`, then 1 only in an isolated run; separate source-only `QueryTimeout=1` vs 0 | Possible submission/claim-tail gain under game load; near-zero expected idle gain when backlog is already small. Measure INPUT_FULL/NEED_MORE_INPUT counts, claim wait, output gaps, CPU time, encode p99 and picture age. Stop at a stall. `amf_quality` submits one picture at a time and cannot establish a queue-depth win. QueryTimeout has no console setting; an experimental build is needed, not a new shipped flag. |
| 7 | `amd_quality=speed,balanced,quality`, starting at 1080p60/low bitrate; optionally `amd_high_motion_quality_boost=auto,enabled,disabled` in separate cells | Low expected return at the owner's normal rates; quality already cost up to 6.1 ms in AV1. Only keep a slower choice if new native content shows a visible gain and encode/picture-age tails still meet 16.67 ms at 60 or 8.33 ms at 120 fps. This is a frame budget, not a guarantee that the full pipeline fits in one frame. |
| 8 | AVC/HEVC `amd_lowlatency_mode=auto,enabled`; AV1 `amd_av1_latency_mode=auto,lowest,realtime`; split `auto,disabled,enabled` | Mostly redundancy checks: effective internal latency/AV1 lowest and split were already selected by the default usage/driver. Read settings first and skip pairs with identical effective state. Compare encode/picture-age p99 and power. Revisit split primarily after a driver change; two reported VCNs alone predict no gain. |

SAV and PA/HQ are last, separate investigations, not recommended changes to
the HDR baseline. Prior SAV/split sweeps were neutral. Do not combine explicit
SAV-on with forced internal low latency; the existing guard documents past
HEVC HDR resets. An 8-bit SDR-only PA comparison would use
`amd_rc=vbr_peak` with `amd_preanalysis=false,true`, then QVBR at 18/23/28 or
HQVBR separately. Inspect PA's effective tools and reference/reorder behavior.
The current one-picture-at-a-time quality probe may time out if a mode needs
future input; such a timeout does not measure quality or throughput. A
pipelined experiment would be separate work. HDR requests for these quality
RC modes currently become peak VBR, so they are not genuine HDR QVBR A/Bs.

For every feasible cell, use 1968x2184/120 at **80 Mbps** (also 30 for a
quality-stressed cell), 2560x1440/120 at **50 Mbps** (also 20), and
1920x1080/60 at **20 Mbps** (also 10). HEVC/AV1 get HDR and SDR; AVC gets SDR.
At the normal rates one-frame budgets are respectively **666667, 416667 and
333333 bits**, not bytes. E.g. native HDR VBV=1 requests about 83.3 kB and
max-frame=2 requests about 166.7 kB before network/FEC overhead.

Use `amf_quality`'s existing `--codec`, `--width`, `--height`, `--fps`,
`--bitrate` (kbps), `--frames`, `--config`, `--out` and HDR `--hdr 1` options
only after reboot. It writes the bitstream and reports non-IDR encode
mean/p95/p99, frame bytes mean/p99 and separate IDR byte sizes; **it does not
compute PSNR, SSIM or VMAF itself**. Score a matched decode against the exact
converted reference with the existing quality harness/FFmpeg. Preserve
bit depth, matrix, range, chroma siting and native-size cropping in both
inputs. Its SDR path requests CSC 0 (601 limited), while HDR uses CSC 4
(2020 limited); references must match. HDR input is planar linear gbrpf32le
scaled to scRGB at 203-nit SDR white, so SDR clips merely converted to HDR
do not test native highlights or wide-gamut content. Include native HDR
material with known source/reference handling; default VMAF alone is not an
HDR appearance metric. Score several clips and scene transitions, not only
the original two upscaled 180-frame samples.

Then alternate A/B/B/A streaming runs, at least three per cell, with the
same display/stream refresh, bitrate/FEC, client and game load. Record
rendered-picture-to-decoded-picture age **mean/p95/p99**, new pictures/s,
host encode/claim/send timing, IDR/recovery spikes, decoder time and errors.
This fixture age excludes client scanout and input latency. Use a separate
hardware-decoding client under game load: loopback decoder starvation was
already measured here and can dwarf encoder differences. For native AV1,
check the known 1984x2186 padding/crop behavior before comparing scores.
Archive effective settings after Init and independently inspect HDR headers
and keyframes; do not call a lower requested rate or absent metadata a
quality/latency improvement.

### Verification of this review's changes

All cargo commands used `C:\src\cargo-one.ps1`, the supplied `rust-env.ps1`
and `CARGO_TARGET_DIR=D:\bp-build\amfreview-target`.

- `fmt --all -- --check`: passed.
- `clippy --workspace --all-targets --locked -j 2 -- -D warnings`: passed.
- The focused core policy test passed. Both new non-hardware regression
  tests also passed in the workspace run.
- `test --workspace --locked -j 2 -- --skip heartbeat_check_finds_the_monitors`:
  failed only in the existing `encoder::tests::encoders_open_on_the_configured_gpu`.
  It expected an error naming the nonexistent configured adapter but instead
  got `no desktop display or hardware GPU`. It stopped during adapter discovery,
  before opening an encoder. The Windows suite reported 179 passed, one failed,
  41 ignored and one filtered; preceding suites passed. The unrelated test is
  unchanged.
- `test --workspace --locked -j 2 -- --skip heartbeat_check_finds_the_monitors
  --skip encoders_open_on_the_configured_gpu`: passed (550 tests, 44 ignored,
  two filtered out; doc-tests also passed). Both new regression tests passed.

Tuning proposals, encoder/HDR bitstream tests and hardware validation remain
deferred until the GPU is healthy. The requested single-skip command needs
a post-reboot rerun before it can be called green.

## October 9 job 7 split and stall diagnostics: host A/B, then a GPU reset

Same setup as the job 4 A/B below (base `92afcce5`, 1968x2184 HDR 120,
240 Hz virtual display, hardware-decoding receiver), October 9 02:32-02:49.
Picture age mean / p95 / p99 ms:

| Build | HEVC rounds | AV1 rounds |
| --- | --- | --- |
| base | 12.40 / 13.12 / 13.39, 12.41 / 13.16 / 13.42, 13.82 / 17.84 / 19.03, 12.42 / 13.24 / 13.44, 12.27 / 12.92 / 13.36 | 11.90 / 12.52 / 12.80, 12.08 / 12.74 / 13.36 |
| job 7 (display/input split) | 12.31 / 12.92 / 13.33, 12.37 / 13.16 / 13.39 | 11.90 / 12.53 / 13.00, 12.02 / 12.66 / 13.15 |
| stall diagnostics | 13.46 / 17.72 / 18.36, 12.38 / 13.14 / 13.40, 12.38 / 13.18 / 13.39, 12.36 / 13.05 / 13.34 | 13.12 / 17.48 / 17.84, 12.01 / 12.63 / 13.20 |

Both neutral: the 17-18 ms p95 tails appear in single runs of both the base
and the stall build (a phase the fixture sometimes locks into), not
systematically. Host mean was 3.6-3.7 ms (HEVC) and 3.2-3.3 ms (AV1) in every
run. Shipped: job 7 as `66547532` and `68273f43`, the stall injection, tests
and stall-time logging as `c35bcd28`.

GPU reset: 25 s into the fifth base run (02:47:11 and again 02:48:03 local)
the Radeon driver timed out (`C:\Windows\LiveKernelReports\WATCHDOG\
WATCHDOG-20261009-0247.dmp` and `-0248.dmp`; the AMD Bug Report Tool opened).
The host log shows audio late reads of up to 0.8 s at that moment, and the
next run's host could not open an encoder within 60 s. The driver did not
recover: the RX 7900 XT shows Code 31 and both virtual display adapters
Code 43, Windows runs on the basic display, and every build's `--diagnostics`
fails with 0x80070057. Over 60 identical runs earlier that night had no
reset; the previous watchdog dumps on this PC are from October 8 00:31-00:43.
Cause unknown: the base build predates every change measured here. It needs a
reboot before any GPU work; the dumps are kept for analysis.

## October 9 a new virtual display hitches other streams

Owner's observation: RX 7900 XT, Windows 11, extended layout, one client
rendering and streaming on its own virtual display. Creating another client's
display raises the first stream's render-to-decode picture age to 500-600 ms
for about 1.3 s. Removing the second display does not hitch. Windows also
reactivates a deliberately inactive physical TV during creation. This is the
reported measurement, not a measurement from this worktree.

Source audit: `59caf6a0`. **Plan only; no display code changed.** The logs do
not establish how many mode/layout applies actually happened, and the existing
code already skips satisfied settings. Combining the remaining operations
crosses identity checks, HDR settling and arrangement recovery bookkeeping;
it is not a clearly safe, contained change without Windows transition tests.
Single-stream and multi-stream behavior therefore remain unchanged.

### Call trace and scope

Entry: `host/src/display_session.rs::Prepared::create` captures the original
layout, calls `Guard::new_virtual_options`, sets virtual DPI, acquires an
arrangement lease, then calls `Guard::apply_virtual_mode("after layout")`.
`GoldenLease` records a restoration snapshot; it does not apply one at launch.
The physical `Activation::acquire` branch is skipped for an owned VDD.

All `SetDisplayConfig` calls below submit the desired **whole active path
set**, even when the code changes just one source/target. They can disturb
DWM/capture on the first display. This is a plausible source of the hitch,
not proof that every call pauses every display or that the driver hotplug
itself is innocent. A device-scoped request likewise is not a guarantee of
uninterrupted composition on other outputs.

In the table, `A` = `SDC_APPLY`, `U` =
`SDC_USE_SUPPLIED_DISPLAY_CONFIG`, `C` = `SDC_ALLOW_CHANGES`. Counts are
display-changing native call attempts, including fallbacks, per invocation
of the indicated step. Read-only queries are not included in apply totals.

| Order / source | Native operation and flags | Scope and count |
| --- | --- | --- |
| Before arrival: `Snapshot::capture`, `hotplug::Protection::capture` | `GetDisplayConfigBufferSizes` / `QueryDisplayConfig` (`QDC_ONLY_ACTIVE_PATHS` or `QDC_ALL_PATHS`); device-name, color, DPI and current-mode queries | Read only; **0 applies**. `ALL_PATHS` includes inactive alternative routes to active targets; these are not dormant monitors. |
| `display_lease` -> `VirtualDisplay::create_options` | Driver version query `DeviceIoControl(0x900)`; one create request (`0x901` for protocol 3.5, `0x90c` for 3.6+) carrying requested dimensions/rate | **1 driver creation**, separate from Win32 configuration counts. Windows/driver arrival and saved-topology recall can reconfigure the desktop before our first apply. Internal commit count is unknown. Shared identity reuse creates no new display. |
| `VirtualDisplay::resolve` -> `activate_target`, only if Windows leaves the new target inactive | `SetDisplayConfig(A|U|C)`, current active paths/modes plus a free-source route for the new target; retry with *all* mode indices invalid and no modes if refused | Normally **0**; **1-2 per activation attempt**. Whole topology; fallback permits choosing every display's timing. Starts after a one-second connected-but-off grace, retried about once a second within the ten-second resolve deadline. Lease renewals are not topology applies. |
| `check_hotplug("created")` -> `Protection::check` | Remove reactivated dormant targets, compact referenced modes, `SetDisplayConfig(A|U)`; on failure retry `A|U|C` | **0 if none reactivated; 1-2 if the TV reactivated**, regardless of number of pruned targets. Full retained active topology, including the first stream; strict attempt preserves its supplied modes, positions, path order and clone relationships. Fallback can retime survivors. Target identity and a second topology snapshot are checked before applying. |
| `Guard::new_virtual_options` -> `lease_hdr` -> `set_hdr` -> `color_state::set` | `DisplayConfigSetDeviceInfo(SET_HDR_STATE=16)` on supported Windows 11 API, otherwise legacy `SET_ADVANCED_COLOR_STATE` | **0-1 HDR write**, addressed to the new adapter/target. Already satisfied or unsupported enable requests are skipped. Successful writes are polled up to three seconds; a timed-out transition can issue **1 additional rollback write** and fail setup. Modern SET failure does not also invoke legacy SET. |
| `finish_hotplug("after settings")` -> `Protection::settle` | Repeated `Protection::check`, same strict/prune/fallback sequence as above | Normally **0 applies**, despite polling: 500 ms quiet period, 50 ms sleeps, 1500 ms enforcement deadline. Each additional reactivation episode adds **1-2 applies**. Sleeps delay this launch; they are not themselves DWM modesets or a demonstrated cause of the other stream's freeze. |
| End of that same `finish_hotplug` -> `VirtualDisplay::apply_mode("after settings")` -> `Topology::set_mode_rate` | If current dimensions/rate differ, edit new source width/height and path refresh, invalidate its target timing index, then `Topology::restore` -> `SetDisplayConfig(A|U|C)` | **0-1 whole-topology apply**. Exact size and refresh within 500 millihertz already count as satisfied. Unrelated paths/modes are supplied unchanged, but `C` allows Windows to adjust them. |
| Mode-list fallback inside that `set_mode_rate` | If CCD succeeded but readback still mismatches, `ChangeDisplaySettingsExW(new_display, ..., flags=0)` with `DM_PELSWIDTH|DM_PELSHEIGHT|DM_DISPLAYFREQUENCY` | **0-1 additional GDI call** to the new display, rounding requested rate to whole Hz. It is not a NULL-device desktop-wide reset, but can still trigger display-change/capture consequences. An error from CCD returns before this fallback. |
| `virtual_scale` -> `set_dpi_scale` | `DisplayConfigSetDeviceInfo(type=-4)`, adapter/source | **0-1 DPI write** for the new source; default configuration 0 and matching scale skip it. Separate from CCD topology applies. |
| `display_arrangement::Lease::acquire`, existing owner -> `apply` -> `Arrangement::compose_all` | Capture current layout, inherit the first stream's arrangement, journal desired layout; `Topology::set_active` only if active ID sets differ | For the stated extended case after successful TV pruning: **0 active-set applies**. In other layouts/state changes: usually **1-2** `SetDisplayConfig(A|U|C)` attempts, as many as **4** when a source reassignment needs an intermediate switch-off and recursive retry. The loose fallback invalidates every source/target mode. |
| Same arrangement `apply` -> `Topology::set_positions` -> `restore` | Place subsequent streams to the right, aligned with the first; `SetDisplayConfig(A|U|C)` if a source position changes | **0-1 whole-topology apply**. Already-correct positions skip it. For two extended streams the intended edit is the second display's source position; the first display is still included, and `C` still allows retiming. Primary/isolated variants may intentionally move other sources too. |
| `Guard::apply_virtual_mode("after layout")` | Same conditional CCD mode set and GDI fallback as the earlier mode pass | **0-1 CCD + 0-1 GDI**. Needed only if Windows changed/refused the requested mode; commonly just a readback after the earlier mode pass. |
| Verification / optional `hdr_profile::Lease::acquire` | Mode/color queries; optional `ColorProfileAddDisplayAssociation` for a selected differing ICC profile | Queries: **0 applies**. Profile: normally **0-1 association write** to new adapter/source, separate from HDR SET and CCD; failure can restore the prior association. |

The two INFO messages `virtual display mode applied` are emitted by readback
in `VirtualDisplay::apply_mode`, including when `set_mode_rate` took its
no-op return. They are **not evidence of two native mode changes**. The
inactive-target warning is before the strict apply (and its last deadline
check); it does not report whether the strict call succeeded or needed the
permissive fallback. The excerpt alone cannot establish a measured count.

Both mode passes target the new VDD, not the first stream's display. The
arrangement records a restoration snapshot but does not restore every
monitor's pre-arrival timing during this second launch. If arrival or an
`ALLOW_CHANGES` fallback retimes the first display, the new display's final
mode check does not repair it; verify the first display separately.

There is no `SDC_SAVE_TO_DATABASE`, `SDC_NO_OPTIMIZATION`,
`SDC_FORCE_MODE_ENUMERATION`, `SDC_TOPOLOGY_*`, `CDS_UPDATEREGISTRY`,
`CDS_NORESET` or `CDS_RESET` on this launch path. `apply_mode` with
`CDS_FULLSCREEN` exists for the separate public integer-Hz `set_mode` helper;
the virtual launch uses `set_mode_rate`, whose GDI fallback has flags 0.
Snapshot rotation/clone restoration and restoration of every saved monitor
are teardown/recovery operations, not an unconditional second-client launch
step. If arrival actually disables the first VDD, its independent heartbeat
can run activation, HDR/mode and arrangement recovery too; those extra calls
must be counted separately when recovery messages occur.

### Counts per second-display creation

Assume one automatic successful VDD activation, one TV reactivation at
`created`, no later reactivation, an extended arrangement with unchanged
active set, and no error unwind or concurrent recovery. Let `M1` and `M2`
be 0/1 for the two actual CCD mode changes, `P` be 0/1 for a position change,
and `F` be 0/1 for the TV restore's permissive retry:

`SetDisplayConfig attempts = 1 + F + M1 + P + M2`.

- Strict TV restore succeeds: **1-4 CCD applies attempted**. A mode that
  needs one correction plus a position change, with matching final readback,
  gives **3**. If Windows already selected the requested mode and placement,
  only **1** remains. These are conditional source counts, not host measurements.
- TV restore needs its fallback: **2-5 CCD attempts**, including the failed
  strict attempt. Each mode pass can additionally need one new-display GDI
  fallback: **0-2 `ChangeDisplaySettingsExW` calls** in total.
- Add **0-1 HDR**, **0-1 DPI**, **0-1 ICC** write if requested and different,
  and always **1 driver-create IOCTL**. These are different API scopes; do
  not add them up as an equivalent number of global DWM stalls.
- Each extra guard correction adds **1-2 CCD attempts**; an explicit
  activation adds **1-2 per attempt**; a changed arrangement active set adds
  the **1-4** described above. Arrival, errors, concurrent user changes and
  the first stream's own recovery prevent an unconditional fixed count.

Removal is asymmetric: it has no create-time Windows topology recall,
startup guard or new-display HDR/mode setup. It can still change the topology:
`Lease::drop` may deactivate the departing target with `set_active`, then
`VirtualDisplay::drop` sends removal IOCTL `0x902` and lease release `0x904`.
No measured hitch on removal does not imply no global call there.

### Minimum-change proposal and why it is deferred

Two corrections to the suggested one-call approach are essential:

- `SDC_NO_OPTIMIZATION` forces the mode change down to the driver for **each
  active display**. Leave it out. `SetDisplayConfig` enables only the supplied
  active paths; passing just the TV/new-VDD paths would switch off omitted
  displays, including the first stream. Supply all surviving paths and edit
  only the intended ones. Use `A|U` first; neither database persistence nor
  permission to alter other supplied modes is needed for an exact temporary
  layout. [Microsoft: SetDisplayConfig](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setdisplayconfig).
- HDR is a separate device-info request, not a field that can be folded into
  a CCD path/mode array. A GDI call naming the new output is device-scoped,
  but does not promise other outputs will remain uninterrupted.
  [Microsoft: DisplayConfigSetDeviceInfo](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-displayconfigsetdeviceinfo),
  [Microsoft: ChangeDisplaySettingsExW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-changedisplaysettingsexw).

The first candidate should be restricted to a **new, independently sourced
owned VDD joining an existing extended stream arrangement**:

1. Retain creation/resolve and the immediate strict dormant-TV correction.
   Do not leave the TV on through a possibly three-second HDR transition.
   Keep identity checks, mode compaction and the bounded startup guard.
2. Keep required HDR/DPI setup and settling. For this joining-display case
   only, defer the `after settings` mode write to arrangement. Keep the
   first/single-stream, reused-display, exclusive/primary/isolated and
   recovery paths as they are.
3. From one fresh active topology, model the new display at its **requested**
   size/rate before composing its final position. Plan the mode and position
   together, retain every unrelated source/target mode, path order and clone
   relationship, discard unreferenced modes and remap indices. Reject a
   shared/clone source rather than editing another display through it.
   If the modeled result already matches, issue no call. Otherwise apply
   that full surviving configuration once with `A|U`.
4. Journal the same intended arrangement before mutation; recheck ownership
   and topology immediately before apply and verify afterwards. A refused
   strict plan needs a fresh query before using the existing fallback
   sequence, never a replay of a snapshot taken before HDR/hotplug. Keep
   readback and a conditional rate correction if the driver does not honor
   the requested timing; do not drop the final verification just to save a
   log line. `ALLOW_CHANGES` remains an explicit compatibility fallback,
   whose potential to retime the first display must be measured.

For the three-CCD example this aims at **two**: immediate TV prune, then
combined new-display mode/position. It cannot remove Windows' own arrival
reconfiguration or a necessary HDR transition. Folding the prune into the
same final call could reach **one** when HDR is already satisfied and the
final arrangement is known at `created`, but currently that information and
its recovery journal belong to a later lease. Delaying protection or moving
that lease into creation is a larger behavior/lifetime change, not a safe
flag substitution. The one-CCD already-satisfied case has no redundant host
apply to remove while still preserving the TV's inactive state.

Risks requiring host validation: known driver rejection of exact layouts
during arrival (the current guard has a fallback for it); missing/invalid
target timings when changing refresh; cloned/shared source IDs; renumbered
GDI names after pruning; HDR recalling another topology; stale snapshots
overwriting a concurrent user layout change; preserving recovery journals
and lock ordering across `DISPLAYS`, `SETTINGS` and the arrangement lease.
CCD has no atomic compare-and-set, so even a final requery cannot eliminate
the last race. Blindly removing `ALLOW_CHANGES` from `Topology::restore`
would also alter physical-display restoration and single-stream behavior.

Before implementing that candidate, add pure planner tests over synthetic
paths/modes and arrangement nodes: all settings already satisfied -> no
apply; TV reactivation alone -> one prune with other modes preserved;
new mode plus position -> one apply; untouched first-stream rational timing,
primary, negative coordinates and clone relationships; compaction of orphan
modes with shared mode indices; owned/replaced/missing target identity;
reject the new target sharing an existing source; changed snapshot ->
replan; requested dimensions used for placement; unchanged single-stream
and non-extended fallback selection. The existing `display/hotplug.rs`
tests cover pruning, compaction, identities and stale-layout detection,
but cannot prove DWM/driver latency or acceptance of a new combined apply.
No unused planner or speculative runtime change was added here.

Apollo/Vibepollo comparison: this checkout has no `src/` C++ host and no
Apollo/Vibepollo display implementation under `third-party/` (only
`moonlight-common-c` and `nanors`). `docs/butterpollo-cpp.md` records the C++
source removal after rc.23. The Rust `resolve` comment credits Vibepollo's
delayed inactive-target activation, but that is not a source audit of its
current flags or call counts. No comparison numbers are invented.

### Verification and owner's next measurement

Validation used the requested `performance-probe\rust-env.ps1`,
`CARGO_TARGET_DIR=D:\bp-build\hitch-target` and `C:\src\cargo-one.ps1` for
every cargo invocation:

- `fmt --all -- --check`: passed.
- `clippy --workspace --all-targets --locked -j 2 -- -D warnings`: passed.
- `test --workspace --locked -j 2`: **failed**, with 539 passed, 2 failed
  and 44 ignored. The unchanged default suite includes two unignored
  desktop/GPU-dependent tests: `display::tests::heartbeat_check_finds_the_monitors_that_monitors_lists`
  failed with `0x80070057`, and `encoder::tests::encoders_open_on_the_configured_gpu`
  failed with "no desktop display or hardware GPU" in this tool session.
- Rerunning that workspace command with
  `-- --skip display::tests::heartbeat_check_finds_the_monitors_that_monitors_lists --skip encoder::tests::encoders_open_on_the_configured_gpu`:
  **passed**, 539 passed, 44 ignored, 2 filtered out; doc-tests also passed.
- `git diff --check`: passed.

The required unfiltered test gate is **not green**; neither the unrelated
tests nor the machine-wide wrapper were changed. The wrapper returned exit
code 0 even for the failed test run, so results above come from Cargo's test
summary, not that wrapper status. Logs are in
`D:\bp-build\hitch-target\clippy.log`, `test.log` and
`test-without-hardware.log`.

No displays or installed service were changed and no streams were started.
Ignored hardware fixtures stayed skipped. The requested default suite
attempted the two unignored probes above; the follow-up explicitly excluded
them. This commit has no runtime difference to A/B.

For the next host run, keep the first client's renderer, codec, HDR, rate,
resolution and capture backend fixed; repeatedly join/leave the second
client with the TV connected but inactive. Record first-stream picture-age
maximum and p95/p99 in a short window around creation, longest interval with
no new rendered/captured picture, and time to recover to baseline. Whole-run
averages can hide the reported 1.3-second event. Compare creation with removal
and, when convenient, a TV-disconnected control to separate arrival cost
from the corrective prune. Test SDR/HDR separately and verify both displays'
actual refresh/size/position/HDR plus the TV's inactive state.

Correlate arrival, the guard warning, both mode stages and layout with
timestamped native call entry/exit, flags, return codes and before/after
topologies (API tracing or temporary measurement instrumentation). The
current INFO logs cannot supply this. A DWM/DxgKrnl trace can distinguish a
composition/driver pause from capture reinitialization or downstream delay.
After a candidate exists, A/B/A/B the same launch sequence and verify that
actual CCD commits decrease **and** first-stream frame gaps/picture age
improve without retiming the first display or re-enabling the TV. If only
one strict prune already occurs, focus the experiment on driver arrival,
HDR and capture recovery before implementing batching.

## October 9 job 4 split and two latency wakes: host A/B

A/B/A/B on the RX 7900 XT host, October 9 02:05-02:16, against their common
base `92afcce5` (which predates the pacer fix, so some runs claim 121 fps on
both sides): isolated extended virtual display at 240 Hz, motion probe at
240 Hz, 1968x2184 HDR 120 fps at 80 Mb/s, hardware-decoding receiver, 35 s
per run. Picture age is mean / p95 / p99 in ms; detect is the host's capture
detection mean / p95.

| Case | Build | Picture age r1 | Picture age r2 | Host mean ms | Detect ms |
| --- | --- | --- | --- | --- | --- |
| HEVC, DDX | base | 12.16 / 12.67 / 13.16 | 12.10 / 12.63 / 12.95 | 3.64, 3.51 | 0.49 / 0.82, 0.53 / 1.11 |
| | job 4 | 12.22 / 12.83 / 13.31 | 12.77 / 14.78 / 17.08 | 3.59, 3.45 | 0.55 / 1.12, 0.47 / 0.99 |
| AV1, DDX | base | 11.86 / 12.50 / 12.80 | 12.39 / 13.43 / 16.69 | 3.22, 3.09 | 0.56 / 1.12, 0.45 / 0.86 |
| | job 4 | 11.85 / 12.46 / 12.70 | 12.40 / 13.36 / 16.70 | 3.22, 3.09 | 0.55 / 1.12, 0.43 / 0.74 |
| HEVC, WGC | base | 12.89 / 13.65 / 16.90 | 12.86 / 13.51 / 15.16 | 3.43, 3.44 | 0.12 / 0.16, 0.12 / 0.16 |
| | wakes (`dfc865c8`) | 12.88 / 13.56 / 15.24 | 12.89 / 13.56 / 16.94 | 3.43, 3.44 | 0.12 / 0.16, 0.12 / 0.16 |

- Job 4 (`Media::start` split into named setup functions, per-frame code
  moved unchanged): same within noise. AV1 matches to 0.1 ms; the HEVC r2
  tail is a run where both host and receiver ran at 121 fps (the old pacer),
  which shifts phase. Shipped as `7e9f8a22`.
- The WGC helper skipping its 500 µs sleep after a delivery (`dfc865c8`)
  changes nothing measurable here: detection is 0.12 / 0.16 ms on both. With
  one frame per 4.2 ms, the next frame is never ready right after a delivery,
  so the skipped sleep never mattered. The IDR/RFI wake (`81c8fb3a`) needs a
  static screen with packet loss to show; that was not measured. Neither is
  shipped yet: both stay on the local `codex/wake` branch until a loss test
  shows the recovery frame coming sooner.

## October 9 pacer claims at the stream rate (bug hunt item 1): host A/B

`stream_policy.rs` refilled pacing credit at 1.01x the stream rate, so a
source faster than the stream (the default 2x virtual display, a 165 Hz game)
was claimed at 121.2 fps on a 120 fps stream and 60.6 on 60. `3309053` (on
main as `75414f78`) refills at exactly 1.0 when the source interval is under
0.95 periods. A/B/A/B on the RX 7900 XT host, October 9 01:24-01:31,
`0b1967d` against `3309053`: loopback with the hardware-decoding receiver,
isolated extended virtual display at 2x the stream rate with the motion probe
at 2x, DDX, HEVC, 35 s per run, `minimum_fps_target` unset.

| Case | Build | Receiver steady fps | Host fps | Picture age mean / p95 / p99 ms | claim_wait p95 ms | Dropped |
| --- | --- | --- | --- | --- | --- | --- |
| 1968x2184 HDR 120, 80 Mb/s | before r1 / r2 | 121.09 / 121.00 | 120.75 / 120.79 | 12.96 / 13.95 / 17.05, 12.94 / 13.90 / 17.12 | 1.04 / 0.99 | 0 / 0 |
| | after r1 / r2 | 120.02 / 120.00 | 119.72 / 119.79 | 13.07 / 13.72 / 17.14, 12.94 / 15.18 / 17.11 | 0.96 / 0.97 | 0 / 0 |
| 1080p60, 20 Mb/s | before r1 / r2 | 60.52 / 60.30 | 60.32 / 59.97 | 7.95 / 12.10 / 19.76, 7.50 / 8.11 / 11.08 | 2.19 / 0.03 | 0 / 0 |
| | after r1 / r2 | 60.00 / 60.00 | 59.56 / 59.65 | 7.62 / 8.26 / 8.75, 10.54 / 11.11 / 11.49 | 0.02 / 1.76 | 0 / 0 |

Result: the stream now leaves at exactly the stream rate (120.0 and 60.0
instead of 121.0 and 60.3-60.5); picture age is unchanged within run-to-run
noise (the 1080p60 runs differ by phase, in both directions). Shipped for the
rate: a client showing 120 Hz received 1.2 extra frames a second, which it has
to drop or queue (Moonlight's 121.0-121.2 received fps in the laptop runs
above). Not measured: Moonlight's own dropped-frame count on a real client
after the change; the loopback receiver decodes but does not present.

## October 8 repeats past the frame cap (PyroWave, VRR): host A/B

Measured on the RX 7900 XT host on October 9 (01:05-01:16), A/B/A/B,
`5e16d1c` (before; same Rust as `9cf57ce`) against `88d1dec` (after).
Loopback with the independent receiver (decoder plus per-frame size in its
timing CSV), not the laptop: an isolated extended virtual display at 200 Hz
(2x the stream rate), 1920x1080 at 100 fps with VRR requested,
`minimum_fps_target` unset, PyroWave at 400 Mb/s and AV1 at 50 Mb/s, motion
probe at 90 Hz on the 200 Hz display (presents on a 5 ms grid, so 10 and
15 ms gaps alternate), 35 s per run, 3 s warm-up dropped. Sent fps is counted
per one-second window from client arrival times.

| Case | Build | Sent fps p50 / max | New frames KB p5 / p50 | Picture age p50 / p95 / p99 ms | Worst gap ms |
| --- | --- | --- | --- | --- | --- |
| PyroWave, 90 Hz game | before r1 | 144 / 151 | 54.8 / 216.6 | 15.52 / 18.59 / 19.51 | 13.5 |
| | before r2 | 150 / 151 | 63.9 / 130.4 | 15.06 / 17.09 / 17.93 | 13.1 |
| | after r1 | 90 / 91 | 340.2 / 494.2 | 14.87 / 17.94 / 18.56 | 18.6 |
| | after r2 | 90 / 91 | 377.5 / 494.2 | 14.67 / 16.90 / 17.63 | 15.9 |
| PyroWave, still desktop | before r1 / r2 | 97 / 100, 97 / 101 | all 193 KB repeats | - | 20.1, 12.0 |
| | after r1 / r2 | 20 / 67, 20 / 82 | all 193 KB repeats | - | 51.4, 51.3 |
| AV1, 90 Hz game | before r1 / r2 | 90 / 91 | 1.3 / 4.4 | 10.76 / 13.83 / 14.74, 10.26 / 12.70 / 13.39 | 17.6, 15.5 |
| | after r1 / r2 | 90 / 91 | 1.3 / 4.4 | 10.58 / 13.04 / 13.78, 10.47 / 12.92 / 13.64 | 15.8, 17.7 |

Result: the report reproduces on the old build and is fixed. Before, a 90 fps
game on a 100 fps PyroWave VRR stream went out at 144-151 fps, because about
40% of the frames were repeats, and new frames were budgeted from the short
gap after a repeat: median 130-217 KB against 494 KB after. After, exactly the
game's 90 fps goes out, every frame new, at full size; picture age is the
same or 0.3-0.7 ms lower. A still PyroWave desktop goes from 97 fps of repeats
to the 20 fps minimum, so the worst gap there is now 51 ms by design (max 67
and 82 fps are the first second, before the picture settles). AV1 is
unchanged within noise, as expected: it already had a 20 fps minimum and no
repeats while the game ran. Not measured: a real client over Wi-Fi (the
laptop), and a real game.

Report (a user, relayed by the owner; rc.25, PyroWave with VRR at 100 fps):
motion windows at 105 to 136 fps. Saving a minimum frame rate of 30 brought
motion to just under 100, a still screen toward 30 and a 34 ms worst gap.
Latency-relevant code change for rc.27; nothing measured on a host yet.

- An unset `minimum_fps_target` was the full stream rate for PyroWave, a
  mis-port: Vibepollo's default of 20 applies to PyroWave as
  min(20, stream rate), and only a saved 0 means the full rate. It is now 20
  for every codec (`stream.rs`).
- Arrival pacing (VRR, `frame_pacing=arrival`) now spends pacer credit on
  every encode, static repeats and same-picture keyframes included. Only new
  pictures did before, so a game frame that came just after a repeat went out
  at once, past the cap. Grid pacing already counted repeats.
- A new PyroWave picture is budgeted at least min(time since the previous new
  picture, one period), never only the gap since a repeat
  (`pyrowave::Interval`); repeats keep the gap since the last encode, and
  critical FEC still caps every frame at one period.
- Model (`repeats_count_toward_the_stream_rate`: a game at about 90 fps with
  9 to 13 ms gaps on a 100 fps VRR stream): with the minimum at the stream
  rate, 158 fps uncounted and 101 fps counted; counted, game frames after a
  repeat wait up to 9.8 ms and 7 of 180 are replaced by a newer one. With the
  20 fps default no repeat goes out while the game runs, so no frame waits.
- Expected costs: after a stall of 50 ms or more, the first game frame can
  wait up to half a period if a repeat just went out. A still PyroWave
  desktop now sends 20 fps instead of the stream rate, so detail FEC
  (`DetailFec`, parity for unchanged blocks when frames come slower than the
  stream rate) now engages there, as in Vibepollo (inferred from the code).

Host A/B to run (idle host, never exclusive display mode): PyroWave, VRR,
100 fps to the laptop client with a renderer below the stream rate with
uneven frame times, then a still desktop. Compare the commit before this
change with this one: sent fps per one-second window (p50 and max), size of
new frames (p5 and p50), picture age, and the worst gap on a still screen.
Repeat once with AV1 for the counting change alone.

## October 8 user report: exclusive layout ignored, picture freezes

Report (relayed by the owner): with the virtual display layout on exclusive
and Remote Monitor chosen on the client, the first physical display stayed on
and primary; and sometimes the picture stops while the stream and its audio
go on. Code changes only; nothing here has run on a host yet.

- Remote Monitor never applied the layout: the arrangement was taken for the
  Stream role only (`display_session.rs`). It now follows which displays stay
  on and which is primary (exclusive and the primary layouts), keeps the place
  the remote monitor layout gives it, restores the layout when its stream ends,
  and reapplies it when the monitor's lease recreates the display (`2180d8f`).
- WGC capture could go silent for good: when the captured display leaves the
  desktop even briefly (a layout change switching it off and on), its capture
  item closes and WGC returns no frames and no error, while the host re-encodes
  the last picture and audio continues. The capture item's `Closed` event and a
  once-a-second monitor-handle check now raise an error, which reopens capture
  on the display that is there now.
- After a reopen, capture recorded the display identity read after opening,
  so a display recreated during the reopen was never followed. It now records
  the identity it opened.
- A recovered virtual display was handed to capture only once its HDR and mode
  were restored; while that kept failing, capture stayed on the old display.
  Capture now follows at once and the restore is retried.

To confirm on the host (never in exclusive mode on the owner's PC): a stream
on a virtual display while another device's virtual display arrives and
leaves, checking that `capture reopened` follows any `left the desktop` error
and the client keeps fresh pictures. Not addressed: a session thread stuck in
a driver call (encoder terminate or init) would also freeze the picture with
audio running; there is no watchdog for it yet.

Host check, October 9 01:36-01:41, build `9cf57ce` (includes `2180d8f` and
`b193b88`), isolated host on the RX 7900 XT with the extended layout, WGC,
HEVC 1920x1080 at 60 fps to the hardware-decoding receiver on a per-client
virtual display at 120 Hz with the motion probe at 120 Hz, 60 s:

- Freeze fix: while that stream ran, three other clients each paired,
  launched Desktop on their own 1280x720 virtual display, streamed 5 s and
  disconnected without quitting the app. The first stream kept running to the
  end: no capture error, no "left the desktop", nothing to reopen. Host fresh
  fps stayed 59.9-60.5 per 5 s window with `wgc_stamp_frames` 242-308, except
  one window at 40 fps with a 195 ms send gap. No run of 20 fps repeats.
  Pass for the freeze.
- But each new virtual display hitches the other stream: picture age rose to
  0.5-0.6 s for about 1.3 s while the second and third displays were created
  (receiver p99 122 ms and max 612 ms over the run, 54 intervals over 1.5
  periods, 46 repeated pictures). The clients leaving caused none. The hitch
  is the age of what the renderer on the first display had presented, so it
  is likely DWM pausing presents during the topology change rather than the
  host's capture; not yet compared with Vibepollo. A first try, where the
  extra clients quit the app on leaving (as the test receiver does by
  default), ended the first stream with them, as expected for a quit.
- Remote Monitor (`2180d8f`): a launch with `remote_monitor=1` and the host
  layout extended logged `role=RemoteMonitor ... layout="extended"
  arrangement=None` and made no display changes beyond creating its own
  display. It also warned "Display refresh is 60.000 Hz for a 60.000 fps
  stream": the remote monitor runs at the stream rate, not 2x as a stream's
  virtual display does. Exclusive and primary were not tried on this PC (the
  owner's monitor would switch off), nor a per-device layout.

## October 8 real-client picture age over Wi-Fi (rc.24, laptop)

First render-to-decode measurement from a second machine instead of loopback.
Host: RX 7900 XT, 2.0.0-rc.24, Desktop app on the Extended virtual display,
`motion_probe` drawing a QPC barcode (240 Hz source on the 120 fps runs, 120 Hz
on the 60 fps runs). Client: IdeaPad, Radeon 780M, driver 32.0.31035.1003,
**on Wi-Fi only** (802.11ax, 5 GHz, 2402 Mbit/s link); host on Ethernet.
Decoder: the independent `rust/tests/moonlight_client.c` built on the host from
`e5520ac0` (FFmpeg D3D11VA for HEVC and AV1, decoding on its own thread, reading
back only a 640x8 barcode strip), launched by a separately paired test client
("lap-measure", Extended layout). Clock: the host pings the laptop every 200 ms
over UDP; the 20% of pings with the lowest round trip give a linear
host-to-laptop QPC map (1,682 pings, minimum RTT 1.16 ms so offset error is
within about +-0.6 ms, fitted drift 17.5 ppm, fit residual at most 0.3 ms).
One 30 s run per row, the first 3 s dropped, first decode of each picture only.
HDR requested on every row; all decoded as 10-bit P010, BT.2020, PQ.

"Received" is when the client's decode thread starts on the picture (fully
received, after its queue). "Decoded" adds the client's hardware decode plus the
synchronous strip readback; Moonlight-qt reports 0.3-0.7 ms hardware decode on
the same laptop, so a Moonlight user's picture age lies between the two columns.

| Config (bitrate) | Fresh fps | Render to received avg / p95 / p99 | Render to decoded avg / p95 / p99 | Host processing avg / p95 | Client decode avg |
| --- | ---: | --- | --- | --- | ---: |
| 1968x2184@120 AV1 HDR (80 Mbps) | 120.0 | 13.54 / 14.33 / 14.93 ms | 18.76 / 19.61 / 20.28 ms | 3.04 / 3.3 ms | 5.21 ms |
| 1968x2184@120 HEVC HDR (80 Mbps) | 120.0 | 14.13 / 14.97 / 15.90 ms | 19.32 / 20.35 / 21.35 ms | 3.70 / 4.1 ms | 5.19 ms |
| 2560x1440@120 AV1 HDR (50 Mbps), repeat 1 | 120.0 | 13.34 / 14.15 / 16.12 ms | 18.58 / 19.76 / 21.49 ms | 2.77 / 3.0 ms | 5.24 ms |
| 2560x1440@120 AV1 HDR (50 Mbps), repeat 2 | 120.0 | 13.28 / 14.07 / 15.63 ms | 18.10 / 18.92 / 20.91 ms | 2.76 / 3.0 ms | 4.82 ms |
| 2560x1440@120 AV1 HDR (50 Mbps), first run (noisy) | 119.5 | 14.78 / 21.87 / 34.85 ms | 20.25 / 29.51 / 45.16 ms | 2.74 / 3.0 ms | 5.47 ms |
| 2560x1440@120 HEVC HDR (50 Mbps) | 120.0 | 13.67 / 14.69 / 19.87 ms | 18.80 / 20.22 / 24.97 ms | 3.11 / 3.2 ms | 5.13 ms |
| 1920x1080@60 AV1 HDR (20 Mbps) | 60.6 | 12.91 / 15.05 / 17.89 ms | 16.88 / 19.40 / 22.18 ms | 2.16 / 2.5 ms | 3.97 ms |
| 1920x1080@60 HEVC HDR (20 Mbps) | 60.5 | 12.99 / 14.15 / 16.62 ms | 16.51 / 17.69 / 20.28 ms | 2.39 / 2.7 ms | 3.51 ms |

Every run received all frames with zero decode failures; unique fresh pictures
matched the stream rate (1440p AV1 skipped 17 source frames and had 183
intervals over 1.5 frame periods, which is its wider p95/p99). Two repeats
at 15:42Z and 15:43Z (14 and 24 late intervals, no skipped source frames)
measured 13.3 ms average and 14.1 ms p95 to received; the host side of the
first run was clean, so its late frames came from Wi-Fi or the client. AV1 is 0.6 ms faster than HEVC to
the client at native resolution, mostly host processing (3.0 vs 3.7 ms).

A first batch at 15:04Z (older client build) is discarded for 120 fps and AV1:
the client decoded and read back whole frames on its receive thread (8-10 ms a
frame), shed about 11-15% of frames and queued for seconds, and its FFmpeg chose
libdav1d without D3D11VA for AV1. Its valid 1080p60 HEVC row (12.91 / 14.82 /
17.22 ms to received) agrees with the table.

Outside this table: a game load on the host and Vibepollo on the same
fixture were measured on the same-PC loopback fixture (see "rc.24 against
Vibepollo 2.0 on the same GPU"). Also out of scope here: Ethernet on the
client, display scanout, and PyroWave (stock and fixture clients lack it on
this laptop). Moonlight-qt averages on the same laptop and host
(rc.21-rc.24): host processing 2.6-3.6 ms, network 1-3 ms, decode 0.3-0.7 ms.
Raw files: laptop `C:\Users\ramaz\bp-measure\phase2b`, host clock pings and
rendered-frame maps in the project files.

## October 8 encoder stall recovery backs off (RX 9070 XT report)

Report: RX 9070 XT host, HD 630 client on Streamlight, 4K60 HEVC 80 Mbps,
rc.22: freeze, then the stream ends, with `encoder_recovery` "returned no
frame for 100 ms". That rule dates from rc.19 (`07a8a55e`); rc.22 only moved
the message onto the stream card. When two frames sit in AMF for 100 ms the
encoder is recreated, and a recreated encoder that was silent for another
100 ms was recreated again, about ten create/destroy cycles a second, until
the 5 s budget ended the session ("the encoder stopped returning frames").
A fresh encoder's first 4K keyframe on the 9070 XT's single VCN beside a game
can outlast 100 ms, so the loop could never let it finish.

Change: the first stall still recreates after 100 ms; each further
recreation without a frame doubles the wait (200, 400, 800 ms), and any frame
out resets the stall timer even if the in-flight count did not drop. The
5 s budget is unchanged and streams without stalls take the same path.
Follow-up: an automatic AMF "safe profile" after repeated stalls (low-latency
usage, no SmartAccess Video, LTR or forced queue) was tried and removed: it
raised latency for the rest of the session on an unproven RDNA4 link, and
overrode the owner's settings. Compute conversion still moves to the
graphics queue only after a second separate failure, as before.

Second follow-up, from the reporter's log: the last timings before the
stall were healthy (59.4 fps, encode 4.6 ms mean, 5.4 ms p99, host max 6.1
ms), the GPU was near 99% with a game, and `network_fec` had fired. The
stall was sudden, not a slow encoder, and Sunshine on the same PC only got
choppy when the GPU was saturated. So a stall now waits 250 ms before the
first recreation (then 500 ms, 1 s, 2 s), the budget is 20 s, and a frame
too large for full FEC in Moonlight's four blocks keeps the parity that
still fits instead of none (849-1019 shards; 900 shards at 20% now get
13%). `wgc_stamp_future_frames` 302/303 with WGC's stamp about 4 ms ahead
is a capture clock offset the timings keep signed, not a cause.
Not measured: the RX 9070 XT is not available, and the RX 7900 XT has not
reproduced the stall. The debug soak's encoder-failure fault exercises the
encode-error path, not this one; a stall injection is still to be written.

October 9 follow-up: the debug soak fault directory now accepts `encoder
stall`, containing a duration in milliseconds (for example `300`), or an
empty `encoder stall.persistent`, which withholds output until removed.
The finite request is consumed once. The fault holds completed frames at
the session's encoder boundary, counts them as backlog, and survives
recreation; pictures from the old encoder are discarded on recreation.
It is compiled only with debug assertions or tests, uses the existing
`BUTTERPOLLO_TEST_FAULT_DIR`, and has no release setting or CLI switch.

GPU-free tests in `host/src/stream/encoder_tests.rs` use a mock at the
encoder-output trait boundary, a virtual clock, the session's actual
recovery helpers and IDR request, and a packetized frame sink. They cover
300 ms of silence followed by a keyframe and more packets, a permanent
stall, the 250/500/1000/2000 ms recreation waits (then capped at 2000), a
short stall released by polling, resetting backoff for a later failure,
and output progress with an unchanged backlog. The 20 s budget starts at
the first recovery attempt, not at the start of the fault. Polling normally
ends it with `the encoder did not produce a frame during recovery`; if the
stall deadline check runs first, the existing message is `the encoder
stopped returning frames`. Both paths are tested, including the failed
session's `0x80004005` termination reason. The sink verifies packet delivery
resumes; this does not exercise a real GPU, network stream or decoder.

Code and local SDK audit (AMF headers version 1.5.2 in
`%BUTTERPOLLO_FFMPEG_ROOT%/include/AMF`; `third-party/` has no AMF docs in
this worktree):

- `amf.rs::poll` calls `QueryOutput` once while an input is outstanding,
  handing one completed frame to the sender immediately. REPEAT, EOF,
  NEED_MORE_INPUT and OK with null data produce no output; it never drains
  a running encoder. `QueryTimeout` is requested as 1 ms (failure ignored;
  the effective settings line shows the actual value). The session polls
  pending output at 100 us where pacing permits, and stops submitting at
  two outstanding pictures. AMF independently drops tracked submissions
  older than two seconds and requests an IDR. These are separate limits.
  At the capped two-second wait, that expiry can lower the backlog first
  and reset the backlog timer; it does not reset the no-output recovery
  budget. Actual recreation spacing also includes initialization and
  refilling the queue: `wait_ms` reports the stall threshold.
- The SDK's `VideoEncoderHEVC.h` documents `HevcInputQueueSize` default 16
  and `HevcQueryTimeout` in milliseconds. `amd_input_queue_size=0` leaves
  the driver queue alone; a positive setting requests 1–32. That is not
  the session's two-picture backlog or the native eight-surface ownership
  limit. Both CPU and GPU SubmitInput loops poll and yield on INPUT_FULL,
  failing after 100 ms; the GPU capacity wait has its own 100 ms limit.
  INPUT_FULL therefore normally gives an **encoding error**, distinct from
  successfully accepted input that never produces output.
- Defaults remain ultra-low-latency **usage**, speed and latency VBR;
  `LowLatencyInternal` is a separate optional setting. PA is normally off
  (quality rate controls can enable it with one-frame lookahead); pre-encode
  can depend on the driver's usage preset. Forced low latency plus explicitly
  enabled SmartAccess Video is already guarded. None of these settings was
  changed. The existing `AMF encoder settings` log reads back queue size,
  timeout, usage, PA/pre-encode, SAV, LTR and hardware-instance count.
- `compute.rs` normally converts RGB to NV12/P010 on D3D12 compute, asking
  for high priority, then normal if denied (global realtime is opt-in).
  Its queue waits for the capture-ready fence before conversion. AMF gets
  the texture in COMMON and a per-texture fence via the GUID contract in
  `core/D3D12AMF.h`. The D3D11 fallback uses the graphics context. Only a
  second *separate* encoder failure disables compute, as before. The host
  requests DXGI thread priority 7 and WDDM process scheduling class 5,
  falling back to 4; these are not an AMF VCN-priority or reserved-capacity
  setting. No encoder-priority property is set by this path.

The SDK contracts agree with AMD's [asynchronous encoder guide](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/wiki/Guide-for-Video-CODEC-Encoder-App-Developers)
and [HEVC header](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/public/include/components/VideoEncoderHEVC.h).
Ranked hypotheses below are inferences, not an RX 9070 XT reproduction:

| Rank | Hypothesis | Evidence that would distinguish it |
|---|---|---|
| 1 | AMF/driver or D3D12 handoff wedges under contention after accepting input. Healthy timings followed by abrupt silence fit better than a steady throughput limit. | `submit_result=AMF_OK`, repeated REPEAT/OK-null with two old inputs, healthy devices, conversion fences completed but AMF's surface fence/ownership not advancing. If all fences complete, the wedge is farther inside AMF/output processing. A WPR/GPUView trace and driver-version comparison are needed to separate these. |
| 2 | Capture-copy or colour-conversion work is starved by the 99% game load, so VCN has no ready input. High queue priority still shares GPU execution/memory resources. | `conversion_completed < conversion_submitted` or a surface's `completed < conversion_value`. Trace the capture fence dependency and compute scheduling with GPUView; a ready conversion weakens this hypothesis. D3D11 input instead needs a graphics-queue trace. |
| 3 | A queue/latency/analysis configuration exposes driver buffering or a driver bug. A two-picture host cap can also conflict with a mode needing more input. | Compare effective InputQueueSize, Usage, LowLatencyInternal, SAV, PA/lookahead, pre-encode and LTR against defaults. Repeated NEED_MORE_INPUT/EOF is distinct from ordinary REPEAT. INPUT_FULL with the explicit 100 ms submit error identifies submission backpressure. The earlier forced-queue RDNA4 report is a lead, not proof about this reporter's settings. |
| 4 | The single VCN is delayed by scheduling, another encoder, or expensive recovery keyframes/pre-encode. | All input fences ready, devices healthy, output eventually resumes as the waits grow. Correlate VCN engine occupancy/other sessions in GPUView and the first recovered IDR; collect under-load encode latency. The pre-stall 4.6 ms mean weakens sustained 4K60 throughput exhaustion. One hardware instance leaves no second instance for split-frame relief. |
| 5 | Device removal/TDR, or surface retention/accounting failure. | Non-success `d3d11_removed`/`d3d12_removed`, a fence completed value of `u64::MAX`, Windows display-driver reset events, or native eight-surface capacity errors. `retained` far above `in_flight` and completed surface fences with excess owners suggest retention; correlate with `encoder_dropped` and queue-drain errors. |

New diagnostics, only on startup, failure/recreation or recovered output:

- `AMF encoder settings` additionally reports `input_memory`,
  `compute_priority` and the read-back `d3d11_gpu_priority`.
- `encoder stall recovery`: `wait_ms`, `recreation`, `recovery_ms`.
- `AMF stall snapshot`: last SubmitInput/QueryOutput numeric codes and
  symbolic `submit_result`/`query_result`, lifetime `input_full` retry count,
  `in_flight`, `retained`, oldest PTS/age, next PTS and D3D11 device status.
  A nonzero lifetime count alone does not locate the current failure.
- `AMF compute progress at stall`: compute priority, last conversion
  submission/completion fence values and D3D12 device status.
- `AMF surface fence at stall`: at most eight target rows with conversion,
  AMF and completed fence values plus texture owner counts. These are
  observations at different instants, not an atomic GPU snapshot.
- `encoder output resumed`: elapsed recovery time. The finite debug
  injection also logs `soak injected encoder stall` with `duration_ms`.

The release frame path only retains AMF result codes and a retry counter
for these snapshots; it adds no timer reads, GPU waits, property writes or per-frame
logs. Timing, queue limits, defaults and recovery decisions remain unchanged.
The software deadlines still require AMF calls to return: a driver blocking
inside QueryOutput, SubmitInput, Init or Terminate can overrun them. If logs
stop entirely, capture the streaming thread's stack/ETW trace; the synthetic
output fault does not simulate a hung driver call. No hardware/ignored tests,
streams or installed-service changes were used for this investigation.

Validation: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --locked -j 2 -- -D warnings` and
`cargo test --workspace --locked -j 2` all passed through
`C:\src\cargo-one.ps1`, using `D:\bp-build\stall-target`.
Workspace result: 546 passed, 0 failed, 44 ignored; all six new stall tests
passed. No ignored tests were run.

## October 8 rc.22 baseline at the owner's settings

The goal is now a finished product measured at the owner's own settings
(1968×2184 HDR, 120 fps, AV1 and HEVC) plus 1080p60 and 1440p120. This is
the host-side baseline every later latency change is compared with.

Fixture: `run-motion.py` from `bench-rc17` copied to
`bench-rc21` in the artifacts folder, unchanged except for size, rate,
bitrate and decoder options. Isolated host started as SYSTEM in the signed-in
session, virtual HDR display at twice the stream rate, WGC capture (the
default), compute conversion on, native AMF at ultra-low latency with
`speed`. Picture age is the timestamped rendered picture to independent
decoding on loopback. Bitrates requested: 80 Mbps native, 50 Mbps 1440p, 20
Mbps 1080p. rc.22 is the installed release (`3adaa1c22345b6fd`). Three runs
per row; the numbers are the means of the runs' mean, p95 and p99.

The independent client must decode in hardware at these sizes: with FFmpeg's
software HEVC decoder (`moonlight-motion-client-dump.exe`, 8 threads) native
120 fps took 7.8-8.1 ms per frame, the queue grew and picture age reached
340 ms mean with a 950 ms p99. The client from `rust/tests/moonlight_client.c`
built with `BUTTERPOLLO_TEST_HW_DECODER=d3d11va` decodes in about 5 ms. For
AV1 it must pick FFmpeg's `av1` decoder (the default, libdav1d, has no
hardware path); `rust/tests/moonlight_client.c` now does that when a hardware decoder is set.

| rc.22, idle | Picture age mean / p95 / p99 (ms) | New pictures/s | Host latency |
|---|---|---|---|
| 1968×2184 HEVC HDR 120 fps | 13.71 / 14.40 / 15.10 | 120.3 | 3.5 ms |
| 1968×2184 AV1 HDR 120 fps | 13.45 / 14.08 / 15.01 | 120.5 | 3.1 ms |
| 2560×1440 HEVC HDR 120 fps | 12.48 / 13.12 / 14.03 | 120.4 | 3.0 ms |
| 2560×1440 AV1 HDR 120 fps | 12.22 / 12.90 / 13.73 | 120.4 | 2.8 ms |
| 1920×1080 HEVC HDR 60 fps | 14.60 / 15.83 / 17.57 | 60.3 | 2.3 ms |
| 1920×1080 HEVC HDR 60 fps beside `gpu_load 45 1000 0 200`, software decode | 33.14 / 41.94 / 44.38 | 59.1 | 2.0 ms |

Native AV1 still decodes as 1984×2186 (AMD's AV1 alignment), so its runs fail
the fixture's strict size check; the timings are complete. The load row uses
the software client as on October 7 (33.4 ms for rc.17 WGC then): with a
hardware decoder on the same GPU as the load, decoding starves and picture
age reaches seconds, which measures the client, not the host. Native-size
rows beside a load therefore need the separate laptop client. An earlier
rc.21 batch matched these idle numbers within 0.3 ms, but some of its runs
overlapped the rc.22 release session's own e2e streams and it is not used.

Artifacts: `bench-rc21\r22-*` (baseline), `bench-rc21\ab-*` (below).

### Reconnect and issue #6 fixes, no latency change

The sign-in, listener and control-timeout fixes committed alongside this
entry were checked against rc.22 in one alternating batch, three runs each:

| | rc.22 | With the fixes |
|---|---|---|
| 1968×2184 HEVC 120 fps idle, mean / p95 / p99 | 13.66 / 14.39 / 15.15 | 13.76 / 14.49 / 15.21 |
| 1080p60 beside the load, mean / p95 / p99 | 33.25 / 42.02 / 44.76 | 32.94 / 42.02 / 44.39 |

Every run decoded all received frames with zero failures. The differences
are within the spread of the runs. The fixes themselves (a reset during
accept, a session that loses its control peer, a 5-10 s Wi-Fi drop, a launch
before sign-in) are covered by code review and unit tests; they still need a
network test on 192.168.4.10 and a signed-out launch on this host.

### Native size beside a game, and two rejected settings

The October 7 load (`gpu_load 45 1000 0 200`, uncapped) starves the
loopback client's hardware decoder at 1968×2184 120 fps: decoding took 8.3-12.6
ms a frame, above the 8.3 ms period, and picture age grew to seconds. Lighter
uncapped loads (fewer draws or iterations) did the same. A game capped at 60
fps (`gpu_load 45 1000 60 200`, 5.3-5.6 ms of GPU work per frame) leaves the
decoder at 4.7-6.8 ms and is the native loaded cell from now on. A game
capped at 120 fps beside 1440p120 sits at the decoder's limit (8.2 ms) and
its runs spread from 17 to 172 ms, so that cell is not used.

rc.22, 1968×2184 HEVC HDR 120 fps beside the 60 fps game, seven default runs
from three batches: picture age 17.2-18.7 ms mean, p95 18.8-21.7, p99
19.3-22.6, 120 new pictures a second. The host's own split shows where the
load costs time: `claim_wait` (copy submitted to encoder claim) rises from
0.06 ms idle to 1.0-3.0 ms mean, p95 6 ms, and `frame_age` from 0.2 to 1.6-3.5
ms. Encoding stays at 3.3 ms. `claim_wait` is bimodal by run (about 1 or 2.8
ms), which follows the phase between the 60 fps game and the 120 fps stream.

Two settings were tried against that wait, alternating with the default in
one batch each, three runs per row:

| Beside the 60 fps game | Picture age mean / p95 / p99 (ms) | claim_wait mean |
|---|---|---|
| Default (compute queue priority HIGH) | 18.11 / 20.15 / 21.51 | 2.81 ms |
| `compute_queue_realtime=true` (GLOBAL_REALTIME granted) | 17.93 / 19.96 / 21.40 | 2.71 ms |
| Default, second batch | 17.64 / 19.08 / 19.93 | 2.22 ms |
| `frame_pacing_source_phase=false` | 18.09 / 20.23 / 21.77 | 2.23 ms |
| also `frame_pacing_predictive=false` | 17.87 / 22.62 / 24.54 | 2.72 ms |

A realtime copy queue changes nothing, so the wait is not the copy queuing
behind the game's compute. Turning off source-phase or predictive pacing
does not remove it either and makes the 95th and 99th percentiles worse.
Both stay at their defaults. Next: trace where the claim waits (the copy
fence, WGC delivery, or the session thread busy in `QueryOutput`), which is
what the sender-thread and AMF-poll work would change.

Artifacts: `bench-rc21\lp-*`, `rt-*`, `pc-*`.

### First remote client: the laptop over Wi-Fi

Moonlight-qt 6.1.0 on a Radeon 780M laptop (driver 32.0.31035.1003, D3D11VA,
panel not HDR) on 5 GHz Wi-Fi, against the installed rc.22 host on Ethernet,
October 8 07:04-07:12 UTC. Desktop app, `motion_probe` drawing on the
streamed display for every run, client "lap" on the Extended layout so the
owner's monitor stays on. Moonlight's whole-session statistics, one 35 s run
per row (averages only, startup included):

| Run | Received fps | Host processing min / max / avg | Decode |
|---|---|---|---|
| 1968×2184 AV1 HDR 120 fps, 80 Mbps | 121.2 | 2.6 / 4.5 / 2.9 ms | 0.37 ms |
| 1968×2184 HEVC HDR 120 fps, 80 Mbps | 121.0 | 3.1 / 13.9 / 3.5 ms | 0.38 ms |
| 2560×1440 HEVC HDR 120 fps, 50 Mbps | 121.0 | 2.8 / 14.9 / 3.0 ms | 0.51 ms |
| 2560×1440 AV1 HDR 120 fps, 50 Mbps | 121.2 | 2.4 / 4.3 / 2.6 ms | 0.62 ms |
| 1920×1080 HEVC HDR 60 fps, 20 Mbps | 59.5 | 1.9 avg, 9.2 max | |
| 1920×1080 AV1 HDR 60 fps, 20 Mbps | 59.8 | 1.6 avg, 8.5 max | |

Network jitter drops stayed at or below 0.13% at 120 fps. The HEVC maxima of
14-15 ms are each stream's first frames, not a steady-state difference
between the codecs: in the host's own 5 s windows during these runs HEVC
peaked at 4.1-4.7 ms native and 3.4-3.8 ms at 1440p, with p99 3.3-4.2 ms, as
AV1. In the 18 loopback runs above, the first keyframe took 12.7-17.7 ms on
both codecs and frames 2-5 sometimes 7-17 ms; after that no frame exceeded
6 ms. Warming the encoder before the first frame would only shorten stream
start. A first 1080p60 pair overlapped the owner changing his monitor mode
(one 48.7 ms frame) and was repeated.

Render-to-decode delay over the network was measured later the same day
(see "October 8 real-client picture age over Wi-Fi" above): the independent
client launches from a paired identity on the laptop, since the host ties a
session to the address that launched it.

The laptop's smoke test on the installed rc.24 (13:17-13:18 UTC, motion on
the streamed display, Extended layout) received 118.7 fps for 1968×2184 AV1
HDR and 120.1 fps for HEVC, with 3.0 and 3.6 ms average host processing.
Moonlight's AV1 maximum was 20.6 ms against 13.4 ms on rc.23. It is again
the stream's first frames: every 5 s host window of that AV1 session peaked
at 3.82-3.93 ms (p99 3.36-3.81 ms), and the HEVC session's at 4.51-4.53 ms.
A first keyframe's time varies from start to start (12.7-17.7 ms in the
loopback runs), so one start does not show a regression.

### rc.24 against Vibepollo 2.0 on the same GPU

The loopback fixture with the pinned Vibepollo 2.0 build
(`day-work-20261002\vibepollo-baseline-build`, `VIBEPOLLO_ISOLATED_BENCHMARK`)
and the installed rc.24, alternating in one batch on October 8 (16:40-16:57
local), two runs per cell, identical settings: native AMF at ultra-low
latency, Desktop Duplication capture for both (as on October 4), a virtual
HDR display, Extended layout, hardware decoding in the client. No game load:
the large October 4 gap (42.4 against 96.4 ms) was beside a GPU load, where
Vibepollo fed its encoder about 24 fresh pictures a second; its idle gap was
13.8 against 16.0 ms, in line with these. The owner's monitor now runs at 2560×1440
120 Hz, and the virtual display came up at 120 Hz instead of twice the stream
rate in every run of both hosts, so the source drew at 120 fps; these numbers
compare only with each other. The monitor's mode was checked before and after
(2560×1440 at 120 Hz both times); Vibepollo's display recovery changed nothing
this time.

| Picture age mean / p95 / p99 (ms), host latency | rc.24 | Vibepollo 2.0 |
|---|---|---|
| 1968×2184 HEVC HDR 120 fps, 80 Mbps | **17.81 / 18.94 / 19.56**, 3.50 ms | 19.97 / 20.43 / 20.87, 4.04 ms |
| 1968×2184 AV1 HDR 120 fps, 80 Mbps | **17.42 / 18.10 / 18.62**, 3.20 ms | 18.88 / 19.54 / 19.90, 3.89 ms |
| 2560×1440 HEVC HDR 120 fps, 50 Mbps | 17.09 / 17.99 / 18.46, 3.08 ms | 17.19 / 17.68 / 17.93, 3.52 ms |
| 1920×1080 HEVC HDR 60 fps, 20 Mbps | **14.67 / 15.50 / 15.82**, 2.46 ms | 21.05 / 21.25 / 21.43, 1.30 ms (one run) |

At the owner's native size rc.24 delivers the picture 1.5-2.2 ms sooner at
every percentile, with 0.5-0.7 ms less host latency; at 1440p the two are
equal. At 1080p60 one Vibepollo run's renderer never started and the other
was 6.4 ms slower, although Vibepollo reports less host latency there
(its host latency excludes part of the path Butterpollo's includes). Every
run delivered all source pictures except one rc.24 AV1 run: 9.5 s in, WGC's
shared texture failed (`0x887A0026`, keyed mutex abandoned), capture
reopened after a 258 ms gap, and afterwards the client read only 53% of the
pictures while the host still sent 119 fps. Its picture age (17.41 ms) is
from the pictures it did read. That recovery path needs a look: the picture
after reopening may not have been the virtual display's. [Later traced to the
fixture: the virtual display moved, Desktop Duplication lost access and
reopened correctly, but `motion_probe` stayed at the old coordinates; fixed in
`aff04350`.]

Artifacts: `bench-rc21\vp-*`, `displays-before-vibepollo.json`.

#### Beside a game

The same comparison with the 60 fps-capped game beside the stream
(`gpu_load 45 1000 60 200`, which leaves the client's decoder room), two
alternating runs per cell, 18:41-18:55 local, Desktop Duplication for both:

| Beside the game, picture age mean / p95 / p99 (ms), host latency | rc.24 | Vibepollo 2.0 |
|---|---|---|
| 1968×2184 HEVC HDR 120 fps | **19.61 / 21.56 / 23.10**, 3.4 ms | 26.96 / 37.62 / 39.25, 8.3 ms |
| 1968×2184 AV1 HDR 120 fps | **17.15 / 18.96 / 22.18**, 3.1 ms | 24.53 / 36.77 / 38.05, 7.4 ms |
| 1920×1080 HEVC HDR 60 fps | 17.84 / 21.78 / 22.84, 2.2 ms | 20.55 / 22.55 / 22.66, 5.7 ms (one run) |

Beside the game rc.24 delivers the native picture 7.4-7.8 ms sooner on
average and about 16-18 ms sooner at the 95th and 99th percentiles; its host
latency stays at 3.1-3.4 ms while Vibepollo's rises to 7-10 ms. Vibepollo also
delivered fewer new pictures (103-118 a second for HEVC, 106-111 for AV1,
against 117-120). At 1080p60 the two are close. One Vibepollo 1080p run is
left out: its source ran at 60 Hz instead of 120. This matches the October 4
result that the gap opens under GPU load, though the load and absolute values
differ. Artifacts: `bench-rc21\vl-*`.

### AV1 at 1968×2184: padding, and why the render-size rewrite is not shipped

The RX 7900 XT (driver 32.0.31041.1004) encodes a 1968×2184 AV1 stream as
1984×2186 even with `AlignmentMode` set to no restrictions, and the frame
header says `render_and_frame_size_different = 0` (FFmpeg `trace_headers` on
a native rc.22 keyframe). Every client therefore receives the padding unless
it crops to the negotiated size, as Moonlight-qt does. The host now logs a
warning once per encoder at such sizes.

Setting the render size in the frame header is possible without touching the
tile data: the flag gains `render_width_minus_1` and `render_height_minus_1`,
32 bits, so everything after it moves by exactly four bytes and the byte
alignment before the tiles is unchanged. A prototype that parses the
sequence header and the uncompressed frame header up to `render_size()`
(`bench-rc21\av1-render\av1_render.py`) rewrote the keyframe correctly:
`trace_headers` reads 1967 / 2183, and libdav1d decodes a picture
byte-identical to the original. But FFmpeg 9.0.2 still reports and outputs
1984×2186 for it with libdav1d: it does not crop to the render size. The
fix would only help a decoder that honours `render_size()`, and none
available here does, so it is not shipped. Check whether the Android
client's MediaCodec AV1 decoder crops to it, or crops to the negotiated
size itself, before revisiting.

### AMF settings per codec: quality at a given bitrate

`windows/examples/amf_quality.rs` encodes clip frames with the stream's
encoder (compute conversion, the host's default settings plus one change) and
writes the bitstream; FFmpeg 9.0.2 then scores it with VMAF and PSNR-Y
against the same frames (`bench-rc21\quality\run-quality.ps1`). Each frame is
encoded on its own, so encode time is submission to output without queueing.
Clips: a 2D arcade game (Teenage Mutant Ninja Turtles HD, 1080p60) and a 3D
shooter (GoldenEye XBLA, 720p30), scaled to 1968×2184 or 2560×1440, 180
frames each, SDR 8-bit. The rows are 1968×2184 at 30 and 80 Mbps (HEVC,
AV1) and 2560×1440 at 20 and 50 Mbps (HEVC, AV1, H.264), all at 120 fps.
Defaults reach VMAF 96.4-99.0 except H.264 at 20 Mbps (93.5 and 79.7).

Range of VMAF change against the default per codec, and the change in mean
and 99th-percentile encode time and in 99th-percentile frame size, first clip
(second clip in brackets where run):

| Setting | HEVC ΔVMAF | AV1 ΔVMAF | H.264 ΔVMAF | Encode mean / p99 | p99 frame size |
|---|---|---|---|---|---|
| `amd_quality=balanced` | -0.56 to +0.11 | -0.29 to 0 | -0.05 to 0 | same | same |
| `amd_quality=quality` | -0.32 to -0.05 | -0.31 to +0.07 | -0.03 to +0.02 | +0.7 / +1.1 ms HEVC, +6.1 ms AV1, +1.9 ms H.264 | same |
| `amd_rc=cbr` | +0.31 to +0.52 (-0.05 to +0.03) | -0.04 to +0.34 (-0.02 to +0.24) | +0.34 to +0.98 (-0.75 to +0.04) | same | +37-67% (+30-58%) |
| `amd_vbaq=disabled` | -0.13 to +0.24 (-0.03 to +0.40) | -0.22 to +0.07 (+0.01 to +0.19) | **+0.25 to +1.03 (+0.40 to +9.42)** | same | +1% (+6%) |
| `amd_high_motion_quality_boost=enabled` | -0.22 to 0 | -0.29 to +0.22 | 0 to +0.12 | +0.2 to +1.1 ms | same |
| `amd_usage=lowlatency` | -0.22 to +0.23 | -0.29 to 0 | -0.11 to +0.10 | +6 to +8 ms | same |

Only one change pays at no cost: **adaptive quantization (VBAQ) off for
H.264**. It raised VMAF in all four H.264 rows, by 1.0 and 9.4 at 20 Mbps,
with the same encode time. H.264 now defaults to it off; HEVC and AV1 keep
it on, where the effect is within ±0.4 either way. Constant bitrate scores
slightly higher on the first clip only because it spends more of the budget
(VBR with low latency stays 5-20% under the target), and its largest
frames grow by a third to two thirds, which costs pacing time on the
network; it stays off. The slower quality presets and the low-latency usage
buy nothing at these bitrates and cost up to 8 ms an encode.

Limits: SDR 8-bit sources (the owner streams 10-bit HDR; the encoder tools
are the same, but the gains were not measured there), two clips upscaled
from 720p and 1080p, and VMAF's default model. A 1 to 2 VMAF difference is
barely visible; the H.264 row at 20 Mbps is the one a viewer would notice.

The same probe then ran in 10-bit HDR (`--hdr 1`, `run-quality-hdr.ps1`):
the clips become linear light with SDR white at 203 nits, are uploaded as
scRGB FP16 like a captured HDR desktop, and the stream's encoder writes PQ
BT.2020; the reference is FFmpeg's zscale conversion of the same frames.
HEVC and AV1 at 1968×2184 (30 and 80 Mbps) and 2560×1440 (20 Mbps), 120 fps,
both clips, 48 encodes. Defaults reach VMAF 95.8-98.8.

| HDR, against the default | HEVC ΔVMAF | AV1 ΔVMAF | p99 frame size |
|---|---|---|---|
| `amd_vbaq=disabled` | -0.30 to +0.90 | -0.03 to +0.31 | -5% to +5% |
| `amd_quality=balanced` | -0.40 to -0.13 | -0.07 to +0.29 | same |
| `amd_rc=cbr` | -0.23 to +0.48 | -0.11 to +0.36 | +10% to +42% |

Encode time did not change with any of them (native HEVC 5.5 ms mean, AV1
4.9 ms). No change is consistent in HDR either, so HEVC and AV1 keep their
defaults; VBAQ stays on for both.

### rc.23 against rc.22

Main at `bad6076c` (`c4a2d51393496851`) against the installed rc.22, the
same fixture, alternating in one batch, Extended layout, hardware decoding.
Since the morning batches the owner's physical monitor runs in a half-width
mode and the fixture adds its display beside it rather than alone; every
cell sits 4-6 ms above the morning's and the runs spread over about 2 ms, so
compare only within this table.

| Picture age mean / p95 / p99 (ms) | rc.22 | rc.23 candidate |
|---|---|---|
| 1968×2184 HEVC 120 fps (3 runs) | 19.69 / 20.77 / 21.75 | 19.75 / 20.92 / 21.65 |
| 1968×2184 AV1 120 fps (3) | 19.00 / 20.23 / 21.06 | 19.22 / 20.26 / 21.01 |
| 2560×1440 HEVC 120 fps (7) | 17.70 / 18.80 / 19.33 | 18.14 / 19.18 / 20.63 |
| 1920×1080 HEVC 60 fps (3) | 15.40 / 17.56 / 19.08 | 14.49 / 15.76 / 17.28 |
| 1968×2184 HEVC beside the 60 fps game (7) | 21.94 / 24.82 / 30.01 | 23.61 / 36.11 / 40.40 |

Run means at 1440p fall into two groups, about 16.6 and 18.6 ms, for both
builds. Beside the game one rc.23 run averaged 32.2 ms; its host log is
healthy (claim wait at most 1.1 ms, encode p99 4.2 ms), so the time went in
the client's decoding next to the load, and without it rc.23 averages 22.2
ms against rc.22's 21.9. No regression is visible on the host side; every
run decoded all received frames without failures. Artifacts:
`bench-rc21\r23-*`.

## October 5 rc.3 release continuation

The user approved WGC compute by default for the next test release after
reviewing the latency/freshness tradeoff below, and authorized publication.
`wgc_compute_copy=false` and the global compute switch remain available.
Shared captures now respect those settings. The fixed 150 ms recovery wait
has been replaced by acknowledgements after each stream releases old GPU
resources; failed reopen attempts still back off.

The reporter can run the candidate but is only reachable through the user on
Reddit, with no reply time known. There is no access to the RX 9070 XT. Do not
close or advertise a fix for the exact 4.7 versus 3.9 ms latency report based
on local RX 7900 XT/wired tests. Validation and release records follow in
PERFORMANCE.md; earlier dated records below describe their original state.

## October 5 continuation from OpenCode

The active OpenCode work moved from `vibepollo` into this `butterpollo-rust`
worktree. Opus shipped rc.2, fixed stream termination on transient UDP send
errors, compared the DDX path with Vibepollo 2.0, and prepared the launch posts.
Its final unfinished task was WGC measurement and optimization, followed by
investigating packet-burst pacing on slower client links.

The last WGC smoke test succeeded as the interactive user but failed under
SYSTEM at `CreateForMonitor` with `0x80070424`. Opus left additional error
contexts in `windows/src/capture.rs`; those are preserved. The service's host
runs as SYSTEM in the interactive session, so this is a host limitation, not
just an isolated-harness failure.

The continuation on `codex/wgc-capture-recovery` shares the startup/recovery
fallback to DDX and investigates event-driven WGC wakeups. Explicit capture probes stay
strict: requesting WGC cannot silently benchmark DDX. Native reconnect testing
also found that revoking `FrameArrived` after the pool closes aborts inside
Windows; notification removal now precedes pool closure and is idempotent.
Pure notification waits improved idle detection but regressed under GPU load,
so notification registration remains opt-in for the probe; production capture
keeps polling. These changes are local and unreleased. The installed host remains rc.2.

Validation: all 177 workspace tests pass (19 hardware-specific tests remain
ignored by the default suite), Clippy passes with warnings denied, and the
release host builds. The explicit native WGC test passes 16 reconnect/COM
teardown cycles. An isolated user-mode WGC stream decodes 715/715 HEVC frames
at 1080p60 with zero failures and nonzero audio. No SYSTEM-context runtime
retest or installation was performed. The quantitative wakeup experiment and
its limitations are recorded in [PERFORMANCE.md](PERFORMANCE.md#october-5-wgc-startup-and-notification-experiment).

Follow-up: service-mode WGC needs a capture helper running as the signed-in
user. DDX fallback does not establish equivalent VRR or generated-frame
behavior. The subsequent WGC compute-copy follow-up below validates content
and synchronization independently before measuring full-stream latency.
The subsequent follow-up adds completion-based packet pacing with wire
overhead and a cap for known local Ethernet links. A late send no longer
creates a catch-up burst. Three core tests cover the cap, delayed sends and
per-datagram overhead. Independent UDP stress tests on the NUC at
`192.168.4.10` cover integrity, real socket overruns and a separately labeled
modeled bottleneck. This wired fixture does not reproduce the reporter's Wi-Fi.

The user identifies the reporter's GPU as RX 9070 XT, with the latest driver.
Their rc.2 log reports one HEVC instance and transient UDP errors 10055/10035.
The 4.7 versus 3.9 ms comparison remains unresolved. Local 7900 XT tests
confirm the existing compute conversion helps under GPU load; disabling
multi-instance encoding has no useful local improvement. An output-wait
optimization improved component and idle results but regressed loaded LAN
host mean from 6.102 to 6.501 ms, so it was reverted. Do not promote it based
only on the favorable component measurement.

A native DDX failure led to finding a missing display-awake request in Rust's
capture worker. Vibepollo holds this request. The workstation's idle timeout is
three minutes; the unchanged snapshot test passed once with a temporary
request, but later failed again even with a moving source and that request.
Its failure remains unresolved; do not attribute it solely to display sleep.
The worker now holds a thread-bound guard which preserves and restores prior
requirements. This does not establish the cause of the reporter's restarts.

The 230-second wired LAN pair crossed the workstation's 180-second display
timeout. The control recorded no fresh capture claims in all ten samples
after 180 seconds; the revised worker recorded fresh claims in all ten.
Both decoded every received picture and the test tone, with zero codec
failures. Both still required two startup DDX restarts. Preserve that open
startup issue and the failing standalone DDX test in the handoff.

Before the compute-copy follow-up, the workspace/native run passed 198 checks
(180 ordinary and 18 native), excluding the known failing AV1 geometry check
and unavailable NVIDIA hardware. The DDX test passes in that active-desktop
state; earlier inactive-desktop failures remain open. Final LAN checks pass
H.264, HEVC HDR, aligned AV1 and explicit WGC at approximately 60 FPS, with
exact geometry, nonblank pictures, test-tone audio and zero decode errors.

The independent C receiver now runs on Linux as well as Windows, optionally
uses hardware decoding with readback of every picture, checks pixel contrast,
and can require a minimum steady frame rate. The NUC passed 1080p60 HEVC but
could only deliver 27.588 FPS in the 4K60 readback case; that is a failed
performance gate, not a successful 4K60 test. Exact unaligned AMD AV1 geometry
was rechecked and still fails for all twelve SDR/HDR/alignment combinations.

Local validation artifacts are in
`C:\Users\ramaz\.codex\artifacts\butterpollo-wgc-20261005`; the NUC fixtures
are in `/home/rambo/butterpollo-tests-20261005`. Complete measurements and
reproduction are in [PERFORMANCE.md](PERFORMANCE.md#october-5-lan-pacing-and-encoder-follow-up).

That earlier release host built and Clippy/format/diff checks passed. Its SHA-256
is `ff9da9483253a3e5b70737ad7e77006e689effc24ebc4519e0a719dcd66880d2`.
The final 4K60 HEVC loopback check decodes 985/985 received pictures at 60.585
steady FPS, but includes startup blank pictures and recovery time. No installed
service or profile was replaced; test-owned processes and receiver containers
are stopped. Changes remain uncommitted on `codex/wgc-capture-recovery`.
`validation-summary.json` records the checks, limitations and source hashes.

### Subsequent WGC compute-copy and startup investigation

The user asked which optimizations were reverted before continuing. The two
reversions remain default WGC notifications and the shorter encoder-output
wait. Neither was re-enabled. Supported AMD WGC capture can use DDX's fenced
compute handoff and compute AMF conversion with `wgc_compute_copy=true`, with
graceful graphics fallback. Default activation was subsequently reverted after
the corrected comparison below; the option remains experimental.

Native WGC verification compares the candidate before the reference: 120
exact frames and 119 changing pictures at idle, then the same under GPU load,
including a snapshot retained after teardown. Eight complete HEVC streams
in alternating order found loaded host mean 8.557 → 1.953 ms and decoded
picture age 49.052 → 40.630 ms. Idle picture age was 31.432 → 31.835 ms.
The strict distinct-frame gate still fails in several control/candidate runs;
do not equate approximately 60.6 transport FPS with 60 distinct pictures.
See the full table and limits in PERFORMANCE.md. This is not a 9070 XT retest.

A second native regression test reproduced compute-sharing fallback losing
the only available frame. It now copies that same frame through D3D11, and
the test passes. The original DDX arrival probe also falsely reported one
frame when it had no samples; actual frame and presentation counts are now
separate. The new startup probe found no DDX frames while Windows reported
the display off, while WGC supplied one cached image. During a cold stream,
raw duplication observed 5120×1440 BGRA → 3840×2160 FP16 → 5120×1440 BGRA,
coincident with two access-loss events. The initiator remains unidentified;
display changes were disabled in the fixture. A warm repeat had no restart.
Do not add a blind delay or reject legitimate dark frames to hide this.

The motion probe's numeric rate previously changed physical refresh. It now
paces only the animation, with before/during/after checks confirming the
physical 5120×1440 output remains at 240 Hz. The initial comparison above used
the old 60-Hz fixture and forced static repeats at 60 FPS. A four-run reversed
comparison found that using production's existing 20-FPS repeat floor restores
59.9–60.0 distinct FPS at idle instead of 44–46, without changing production.

Eight further streams with production's repeat floor and the corrected fixture
found compute reduces loaded picture age from 52.859 to 34.603 ms, but distinct
FPS drops from 51.423 to 49.955. All four loaded runs fail the 58.2-FPS gates;
all four idle runs pass. This prompted reverting the compute default too.
Keep the option available, the failures visible, and the user's RX 9070 XT
acceptance open. Reports: `repeat-cadence`, `wgc-compute-abba3`.

The moving-desktop workspace/native run passes 200 checks, including 20 native
checks. The unavailable NVIDIA test and known failing AV1 geometry test remain
excluded. The final retained build and validation are recorded in
`validation-summary.json`; the installed service and profile remain unchanged.
Final retained host SHA-256:
`c8341cefcb1cdfc50f8038e735c412175571222a743da5f0ba6ec6a388366959`.
The final run again passes all 200 available tests. Wired default WGC HEVC,
opt-in compute HEVC and opt-in compute HEVC HDR all deliver approximately 60
distinct FPS with zero decode errors. An absent-motion negative check correctly
fails despite decoding all 724 received pictures. Reports: `retained-native-final`
and `retained-lan-final`. All workspace release binaries and Clippy pass.

## October 3 installed state (historical)

Installed revision: `8db29eee4` (October 3, 07:37 UTC; `butterpollo.exe`
SHA-256 `DB543650FC4824E0CDA61919E619E6419106E7B5D45A815AD0C541CB816ACA77`,
58 package files, profile preserved). It contains the scheduling work below
and the multi-stream and display fixes of October 3.
Push only to the owned `butterpollo` remote. Draft PR:
https://github.com/RamazanKara/Rubylight/pull/1.

### Why the customer saw higher host latency than Vibepollo

1. Different counters. The C++ host sends Moonlight
   `frame_processing_latency = send - host_processing_timestamp`, taken when it
   picks the frame up (`src/stream.cpp:2037-2046`, `display_wgc.cpp:384`). The
   Rust host sent `send - presentation`, which adds the 2-3 ms a frame waits
   after Windows presents it. The Rust host now reports claim to packet, as the
   C++ host did, and logs the waiting separately (`frame_age`, split into
   `detect` and `claim_wait`), so it stays visible.
2. Real waiting. The encoder claimed frames on a fixed 120 Hz grid unrelated
   to presentation, so frames aged 2-3 ms before encoding (phase lottery: the
   value was fixed per session). Frames are now claimed on arrival
   (`frame_pacing = arrival`, default; `grid` keeps the old scheduler).
3. Coarse waits. Waitable timers on this PC wake 0.3-0.5 ms late for short
   waits (`windows/examples/timer_probe.rs`), so the 100 us AMF output poll was
   really ~0.5 ms and deadline claims were late. AMF now waits in the driver
   (`QueryTimeout = 1`, only while a frame is in flight); streams raise the
   timer resolution, opt out of power throttling and use high priority, as
   the C++ host did.

### Measured (October 2 evening, same binary, A/B)

Fixture: `day-work-20261002/run-motion.py av1 <label> rust physical-strip-motion`
with `BUTTERPOLLO_TEST_STREAM_SCALE=0.5` (physical 5120x1440 at 240 Hz
streamed at 2560x720/120 so the local software decoder is not the bottleneck),
`BUTTERPOLLO_TEST_DECODER_THREADS=4`, `BUTTERPOLLO_TEST_FRAME_PACING`.
`received_age.py` reports picture age minus client decode time (renderer to
fully received frame), which excludes the loopback decoder's CPU contention.

| Run (v6, final) | Picture age mean / p99 | Received age mean / p99 | Present to send | Frame age |
| --- | --- | --- | --- | --- |
| grid | 12.55 / 13.66 ms | 9.37 / 9.86 ms | 4.8 ms | 2.9 ms |
| arrival a | 10.40 / 11.74 ms | 7.10 / 7.85 ms | 2.5-2.6 ms | 0.40 ms |
| arrival b | 10.29 / 11.51 ms | 7.08 / 7.84 ms | 2.55-2.6 ms | 0.41 ms |
| arrival c (stopped when the customer connected) | - | 7.10 / 7.88 ms | 2.5 ms | 0.41 ms |

Zero late intervals and 120.0 unique fps in every run. Arrival pacing is
2.3 ms faster end to end than the grid on the same binary. The 1 ms timer also
helps the local renderer and client, so do not compare these absolute values
with runs before `d9a04bbb0`.

Three pacer defects were found with per-claim traces
(`BUTTERPOLLO_TEST_RUST_LOG=info,pacing=trace`, `claims.py`) and are covered
by tests: the session-sampled cadence estimate drifted (now the capture
worker's median interval); a fresh frame presented just before the claim slot
but detected after it lost to the older one; and a credit deficit from startup
persisted for a whole session at exactly the stream rate (credit now refills
1 % faster, claims need 7/8 of a frame of credit).

### Against Vibepollo 2.0 (pinned C++ baseline)

Same fixture, same scaled 2560x720 AV1 stream and client, run alternately
after the Rust build above was installed (22:32 local). The C++ baseline's
first run failed (its virtual display restart was denied, the renderer timed
out) and its display recovery re-enabled the HISENSE monitor and set the
Odyssey from 240 Hz to 120 Hz; the second pair therefore ran on a 120 Hz
source. The displays were put back afterwards (Odyssey 5120x1440 at 240 Hz,
HISENSE detached, as before the test). Do not run the C++ baseline fixture
on this machine again without isolating its display recovery.

| 120 Hz source | Rust | Vibepollo 2.0 |
| --- | --- | --- |
| Moonlight host latency (claim to send, both) | 2.05 ms | 2.47 ms |
| Picture age mean / p99 | 15.6 / 18.7 ms | 18.2 / 29.9 ms |
| Received age mean / p99 | 11.6 / 12.5 ms | 13.2 / 22.6 ms |
| Intervals over 1.5 periods in 30 s | 1 | 17 |

One pair only; repeat on the customer's real sessions before claiming it in
release notes.

### Build loop warning

The WSL clock ran 43 s behind Windows, so cargo on Windows skipped rebuilding
files edited in WSL within that window. `systemd-timesyncd` in WSL was stopped
and the WSL clock set from Windows. If edits seem to have no effect, compare
`wsl date` with Windows time and touch the sources.

## October 3: multiple streams and display restoration

The customer reported that multiple streams did not work. Fixed in
`c23384eae`..`8db29eee4`:

1. Reconnect lockout. A client launching again after an abandoned launch or
   stream got "client already has a session" until the old one timed out
   (four failures in a row at 05:09 UTC). A launch now supersedes the same
   client's launch or stream in that role and waits up to 5 s for its teardown.
2. Second client refused. A different display mode ("another stream owns a
   different display mode") or a running arrangement refused the second
   stream, and clients shared one retained display. Each client now has its
   own retained display, a mode set by one stream is kept (the other stream
   scales), and the arrangement is shared and re-laid out as streams come
   and go.
3. A second client's virtual display broke the first client's DDX capture:
   `0x887A0026` on every re-created duplication, 117 restarts, never
   recovered. Windows keeps returning the stale adapter while any device on
   it is alive (`multi_ddx_probe` reproduces this). The capture worker now
   drops the lost capture, withdraws the frame and waits 150 ms for streams
   to release their encoder before re-creating it. The first stream recovers
   after one or two restarts, about a second without frames.
4. RTSP refusals and failures were logged at debug; now warnings.
5. Display restoration. `Topology::set_mode_rate` trusted SetDisplayConfig:
   the HISENSE TV asked for 1080p120 came back at 60 Hz with no error, and
   re-applying the unchanged layout (`set_positions`) did the same. It now
   skips a display already in the mode, verifies the result, and falls back
   to the display's mode list. The layout recorded for restore no longer
   contains the stream's own virtual display (a retained one stayed as an
   invisible monitor beside the Odyssey; restoring its settings after it was
   removed is the likely source of the customer's "os error 31"). Restore
   continues past a failing display, names the failed step, re-checks rates
   after moving displays, and retries briefly while Windows applies another
   change ("cannot read display mode").

Fixtures (in `day-work-20261002`): `multi_stream.py` runs two paired clients
against an isolated host (concurrent join, retry after an unconnected launch,
resume after a client crash). `run-system-multi.ps1` runs it as SYSTEM so each
client gets its own virtual display (`-Layout extended`). The check
`capture_restarts_bounded` allows up to four capture restarts when a display
arrives. `restore_probe` re-applies the current layout, optionally step by
step, and prints every display's rate.

| Run | Result |
| --- | --- |
| Physical display, all scenarios | PASS, 0 capture restarts |
| SYSTEM, per-client virtual displays, before the fix | FAIL, 117 restarts |
| SYSTEM, per-client virtual displays, final | PASS, 1 restart; A 58.1 fps at 60, B 114 fps at 120; layout and rates unchanged; no restore warning |

The arrival fixture could not be compared with the 240 Hz runs above: the
Odyssey was at 120 Hz and the phone's retained 240 Hz virtual display was
attached. Same display state, installed `f44de5dd8` against the new build:
received age 13.6 / 19.8 ms against 13.2 / 20.2 ms (mean / p99), host p99
2.6 ms for both. No pacing regression.

Display state found on October 3 (not changed back without the customer):
Odyssey 5120x1440 at 120 Hz (240 Hz on October 2), HISENSE active at 120 Hz
until `restore_probe` reproduced the 60 Hz bug on it several times; it was
returned to 120 Hz and then went inactive about a minute later, while no
probe ran (TV standby or switched off; still connected). After the update
restarted the service it was active again at 1920x1080 at 120 Hz in its
usual place, and the retained virtual display was gone. The Odyssey is still
at 120 Hz.

Open: the customer's own configuration (exclusive layout, HDR virtual
display) was not run here because it turns the physical monitors off. If
error 31 still occurs, the warning now names the display and step.

## October 3 evening: Vibepollo 2.0 parity

Goal from the customer: Butterpollo replaces Vibepollo 2.0 for its users,
with the latency wins kept and Vibepollo's behaviour everywhere else.
`docs/features.md` is the current list. Done since `967fe6a87`:

- Setup (`rust/setup`): `butterpollo-setup-<version>.exe` upgrades a
  Vibepollo installation in place (drivers as SYSTEM, service, firewall,
  shortcuts, uninstall). Installed on this PC; the host name is now `homepc`.
- New web console (`rust/web`, Svelte 5); the host serves it from
  `assets\web` and keeps the server-rendered pages as a fallback.
- Vibepollo replies for `/bitrate` (capped by `max_bitrate`), `/unpair` on
  HTTP, ABR capabilities, the applist placeholder and
  `VirtualDisplayDriverReady`.
- Device `display_mode`, the config override allow-list, Vibepollo's
  `frame_limiter_auto_virtual_framegen` spellings, `--creds`, credential
  folder permissions at start, the display restore hotkey, an invalid
  `apps.json` no longer stopping the host, and letterboxing in the software
  encoders.
- Steam library sync (`core::steam`, `host/src/steam.rs`): verified against
  this PC's 17 installed apps in three libraries; covers from the cache or the
  store. A Steam app's stream follows the game's processes.

- Playnite (`core::playnite`, `host/src/playnite.rs`) through Vibepollo's
  plugin, and Lossless Scaling (`core::lossless`, `host/src/lossless.rs`),
  both tested against stand-ins because neither is installed on this PC
  (the customer's profile has Playnite apps and the fullscreen entry).
- Tray notifications for pairing and new versions; release checks compare
  versions and skip streams.

Still missing: the virtual display render GPU and reclaim after restart,
Playnite focus retries and fullscreen relaunch, `/api/browse`.

Unverified here: the secure desktop during a stream, streaming the sign-in
screen after a reboot, a Steam game ending its stream, the restore hotkey.

## October 4: latency beside a game

Customer request: the lowest possible encode and end-to-end latency. Encode
is at the VCN floor when idle; the remaining cost was D3D11 work queued behind
a game on the graphics engine. Captures are now copied and converted on D3D12
compute queues and AMF encodes from D3D12 (`cc05d5018`, details and numbers in
`PERFORMANCE.md`). Idle picture age fell 0.6 ms; beside a heavy GPU load it
fell 7-9 ms with more new pictures per second. `gpu_compute_conversion = false`
restores the graphics-queue path.

Probes (`156c9f348`): `gpu_load`, `d3d12_probe`, `copy_probe`,
`ddx_sync_probe`, and `performance --live-capture --arrival` for
present-to-bitstream timing. Scripts in
`<artifact>\day-work-20261004` (`load_matrix.py`, `e2e-latency.ps1`,
`ddx-sync.ps1`, `live-latency.ps1`, `guarded.py`, `quiet.py`).

Lessons for test runs:

- Run GPU tests through `guarded.py`: it refuses to start unless the
  installed host has been quiet for two minutes and kills the test the moment
  a client launches. An idle check right before a test is not enough; a
  customer session started a second after one passed.
- Do not start the isolated host while the installed host has an app running
  (`RustHostApplicationActive`). Its virtual display heartbeat then failed
  with ERROR_BUSY and the customer's exclusive layout was replaced by the
  physical displays. `quiet.py --host` checks this.
- This PC sleeps when idle; a long run may resume hours later.

Next: AMD AV1 still pads 1968×2184 to 1984×2186 (AMF issue 423). Only AMD
GPUs use the compute queues; NVIDIA and Intel keep the graphics-queue copy
and conversion until the path is tested there.

Waiting in Desktop Duplication instead of polling was measured and rejected
(`examples/ddx_arrival_probe.rs`, `day-work-20261004\ddx-wait`, `e2e11`,
`e2e12`). A blocking `AcquireNextFrame` returns a frame 0.08 ms after its
present (p95 0.11 ms) against 0.39 ms (p95 0.89 ms) when polling every
0.5 ms, but it holds the device's immediate-context lock: another thread's
`Flush` meanwhile took 1.1 ms at the median and up to 56 ms (texture
`GetDesc` and output `GetDesc1` are unaffected). With AMF on compute queues
nothing else used that context, and idle streams gained 0.16 ms of picture
age over four runs. Beside the game load, waiting was worse in all three
pairs: 30.6 against 29.2 ms picture age, 91 against 94 new pictures per
second, 0.7 ms more host time. Capture keeps polling.

## Next work

1. Install the current source while the host is idle (package with
   `build.ps1 -Dependencies <artifact>\bootstrap-sdk -TargetDirectory
   <artifact>\target -NvidiaRoot <artifact>\ngx-sdk -MsvcSdk
   <artifact>\msvc-sdk -Package` after dot-sourcing
   `performance-probe/rust-env.ps1`). Check the customer's next 1968x2184 HDR
   sessions: `frame_age`, `host_mean` (now claim to packet) and the Artemis
   HDR10 warning after a host restart.
2. Repeat the A/B against the pinned Vibepollo baseline with the scaled
   fixture (picture and received age), now that the counters agree.
3. Encode is 0.2-0.3 ms slower when claiming right after composition (the
   conversion waits for the capture copy). Investigate converting directly from
   the duplicated surface.
4. AMD AV1 padded decoded size remains open (see below).

## Operating rules

- Windows only; production host and service code in Rust.
- Do not change codec, dimensions, fps, HDR, bitrate or quality to claim a win.
- Check `http://127.0.0.1:47989/serverinfo` AND the installed connection ledger
  before GPU tests and updates. Public FREE/currentgame are scoped to the
  requesting client and cannot prove global idleness. The new loopback counts
  are not available in the installed revision. Stop owned test workloads if
  the customer connects; preserve their stream.
- Preserve profiles, pairing and credentials. No password reset is authorized.
- Routine idle-safe updates are authorized. Do not add project approval gates.
- Preserve timestamps. Report capture-to-delivery and frame intervals alongside
  host processing, so a prettier counter cannot conceal older frames.
- Release only at the customer's request (2.0.0-rc.1 on October 4), and name
  unverified hardware and missing features in the release notes.

## Work log

- 12:30–12:42 UTC: recovered current state, inspected the real customer session,
  capture and encoder scheduling, and previous native acceptance fixtures.
  Arranged a 15-minute follow-up in the current chat. No code optimization yet.
  Follow-up ID: `butterpollo-latency-and-smoothness`.
- 12:42–13:20 UTC: built a native Rust moving HDR renderer and an independent
  decoding client with in-picture QPC timestamps. Removed three-frame buffering
  from the test decoder; this corrects measurement and is not a host improvement.
  Tested 1968 × 2184 HDR at 120 fps. The renderer runs on the virtual output at
  240 Hz, matching its automatic 2× frame-generation mode. Steady loopback
  delivery is 120 fps with no repeated pictures or intervals over 1.5 periods.
  AV1 still decodes to 1984 × 2186 instead of the requested size; strict
  dimension checks fail. An AMF 8×2 alignment request did not fix this hardware.
  A transient HEVC startup failed with a keyed-mutex error; a retry succeeded.
- 13:20–13:29 UTC: context handover and elapsed gap; do not count this as active
  implementation or testing time.
- 13:29 UTC onward: archived current Vibepollo revision
  `8a8c4b03a280ab9f567beb380110abb80f5220b8` with 15 exact pinned dependencies
  for a full-stream comparison. Added same-size shader fast path, GPU timestamp
  measurements, and p99/completion-interval diagnostics. No candidate installed
  or pushed yet.

### Measurements from this pass

Artifacts are in the `day-work-20261002` directory above. These are local test
results, not acceptance of a better customer experience or release readiness.

| Case | Mean / p95 / p99 / max host ms | Mean / p95 / p99 / max picture age ms | Result |
| --- | --- | --- | --- |
| 5d5 HEVC, low-delay decoder | 6.158 / 9.5 / 9.8 / 13.5 | 16.307 / 20.220 / 21.968 / 27.133 | 120.001 fps, strict decode passed; startup probe overlapped |
| 5d5 AV1 | 5.630 / 8.9 / 9.3 / 12.5 | 14.498 / 18.305 / 18.859 / 21.749 | 119.999 fps; wrong decoded dimensions |
| AMF 8×2 request, AV1 | 5.542 / 8.9 / 9.3 / 12.7 | 14.386 / 17.954 / 18.453 / 21.995 | 120.000 fps; wrong decoded dimensions; full startup probe awaited |
| Same-size shader and explicit AMF crop, AV1 | 6.785 / 8.7 / 8.8 / 9.2 | 15.490 / 18.013 / 18.338 / 19.066 | 120.002 fps; wrong decoded dimensions; full startup probe awaited |

Picture age uses the timestamp rendered into the picture and the decode
completion QPC on this machine. It includes local rendering, DWM, capture,
encoding, loopback and software decoding, but excludes remote display scanout.
Vibepollo starts its reported host timer after acquiring a frame; Rust uses its
source timestamp. Those counters alone cannot establish a fair comparison.

Before the same-size shader change, 64 GPU samples measured HDR FP16-to-P010
conversion at native size: 0.335 ms mean, 0.337 ms p95, 0.343 ms p99. Resizing
to 3840 × 2160: 0.520 ms mean, 0.619 ms p95. These isolate conversion and exclude
capture, encoding and delivery; any component gain must be checked in a stream.

After the same-size shader change, native conversion measured 0.170 ms mean,
0.175 ms p95 and 0.179 ms p99. Scaled conversion measured 0.532 ms mean and
0.641 ms p95, versus 0.520/0.619 before; verify this small variance if further
shader changes affect resizing. All six native GPU checks passed, including
absolute HDR luminance/gamut/linear resizing, SDR range/matrix, cursor blending,
4:4:4 chroma, retained texture ownership and conversion timestamps.

The stream result still depends strongly on capture/timer phase: the last AV1
test had a lower maximum but a worse mean. Do not call it a host-latency win.
Its capture-age estimate p95 remained approximately 5.1 ms; encoding p95 was
3.7 ms. The 8×2 alignment experiment was reverted. Explicit surface crop was
ported from the C++ AMF backend but did not fix decoded dimensions on this GPU.

The archived C++ baseline was built successfully with streaming/capture/codec
paths unchanged. Test-only patches guard four machine-wide startup recovery
operations and add an isolated-display creation marker. PyroWave/WebRTC and
driver packaging are disabled in this comparison build; native AMF/DDX remain
the compared paths. Its initial launches failed before streaming because the
portable asset working directory and log parent were missing; fix the fixture,
not the installed host. Preserve both patch files and the pinned inventory.

The GPU process scheduling class was already ported, but per-device GPU thread
priority and maximum render-queue latency were missing. Added the previous
host's relative priority 7 and queue hint 1 as independent best-effort settings.
These compile but await a measured full stream. They are rendering hints, not
evidence that the capture/codec pipeline previously queued three frames.

### Later work, 14:36–15:12 UTC

Continued active coding and testing. No candidate has been installed or pushed.

- Pointer-only DDX snapshots now retain immutable desktop pixels and update
  the cursor separately, avoiding a full desktop GPU copy. A missed new desktop
  invalidates that cache before a later pointer update can reuse it. The native
  test passes for retained pixels, a full eight-texture pool and recovery.
- Added a bounded freshness-wait experiment, **disabled by default**. It waits
  at most half a frame period (capped at 4 ms) and drains encoder output while
  waiting. It is not yet rate-aware or measured, so do not promote the default.
- Added a known quiet tone on the same Steam Streaming Speakers endpoint for
  both hosts. Vibepollo delivered 6,837 audio packets, peak 0.0492 and RMS 0.0347.
  Its earlier zero-packet silent case was not evidence of broken audio capture.
- C++ baseline source refresh still ran at 120 Hz, even after requesting 240 Hz:
  its isolated display helper launch failed with Windows error 5. The requested
  virtual display was created, but its active mode remained 120 Hz. The fixture
  now lets the test-owned renderer set the owned display's mode and verifies
  actual QPC presentation intervals. That matched run has not executed yet.
- C++ at actual 120 Hz source, HDR AV1 1968×2184/120 fps/80 Mbps request, with
  the tone: 120.001 fps, zero late intervals or repeated pictures; host counter
  mean/p95/p99/max 3.514/3.9/3.9/4.3 ms; picture age 17.618/18.730/19.170/19.782
  ms. Source refresh differs from the Rust cases above, so this is **not** a
  valid performance acceptance comparison. Strict AV1 dimensions still fail.
- Automatic approval review rejected the next elevated benchmark launch with
  “blocked by policy.” Do not retry the denied elevated action through another
  mechanism. Native probes and code work continue without elevation. Installed
  service and credentials are unchanged.
- Added a non-elevated native AV1 geometry fixture. Twelve cases (1920×1080,
  1968×2184, 2184×1968; SDR/HDR; alignment modes 3/4) encode and decode eight
  frames each. The driver reads back the requested size and alignment, but
  actual decoded sizes are 1920×1082, 1984×2186 and 2240×1968 respectively.
  Independent traces show the enlarged sequence size and
  `render_and_frame_size_different=0`. No crop/render correction is present.
  The strict test deliberately fails; do not weaken it or claim an AV1 fix.
  See `day-work-20261002/av1-geometry/geometry.json` and header traces. AMD's
  issue 423 documents this family of padded-resolution behavior:
  https://github.com/GPUOpen-LibrariesAndSDKs/AMF/issues/423.
- Found synchronous driver renewal and monitor enumeration under the same
  `Ready` lock read by the capture worker every 100 ms and the encoder every
  250 ms. Changed realtime readers to a short published snapshot of output and
  generation. Maintenance finishes before publication; leases remain owned
  until worker teardown. Retained output/generation are now read together.
  Added a concurrency check for readable old state during a blocked refresh,
  then atomic publication of recovered identity. Full workspace tests and
  Clippy pass; native/customer improvement is unproven.
- A non-elevated read-only maintenance probe sampled the primary physical
  display twenty times, one second apart. Monitor enumeration cost 1.558 ms
  mean, 2.605 ms p95, 4.333 ms maximum. HDR metadata reads cost 0.00196 ms mean
  and 0.012 ms maximum, so an extra HDR polling thread is not justified.
  Enumeration can consume half an 8.33 ms frame slot under the old shared lock;
  removal of that dependency still needs full-stream validation. This probe
  excludes privileged renewal, capture, encoding, network and decoding.

### Regression and package validation, 15:12–15:36 UTC

Continued active work; no candidate has been installed or pushed.

- Full locked release workspace tests pass: 130 ordinary tests. All-target
  Clippy with warnings denied, formatting, release build, separate MSVC NGX
  adapter compilation and packaging pass.
- Twelve available native tests pass together in 58.29 seconds. NVIDIA tests
  and the known failing AV1 geometry gate are explicitly excluded. The geometry
  gate remains a recorded failure, not silently converted into a pass.
- Package verification passes: all 58 manifest entries match staged files and
  ZIP payloads, ZIP integrity passes, and no JavaScript/TypeScript is packaged.
  Package SHA-256: `facedca934ab7bbc2bb409cf1e4ac1881b500c2e3d3d2e233bcd61b6b60542dd`.
  Host SHA-256: `2e6ea482cda5dc4de86a46bcdb032c49273e17d8fb9abfe7d0a8c5e9d8ce9c25`.
- Normal-user physical-contract fixture passes encrypted HEVC Main10,
  permissions, exact 1968×2184, BT.2020/PQ and a known quiet tone through
  WASAPI/Opus. It changes no display settings, uses a disposable profile and
  terminates only owned test processes if the installed customer connects.
  After five seconds warmup: 900 frames, 119.993 fps, zero intervals above
  1.5 periods; host mean/p95/p99/max 5.517/5.9/6.3/6.4 ms; arrival p99/max
  9.198/9.375 ms; audio peak 0.049211/RMS 0.034739, zero decode failures.
  Source is unchanged physical 2560×1440 SDR, scaled/converted to the requested
  HDR format. This is compatibility evidence, not native HDR motion acceptance.
- Packaged remembered-login/legacy-session/API-key migration fixture passes
  all twelve checks across four restarts. It writes only its disposable profile;
  installed credentials remain unchanged.

- The same non-elevated C++ HEVC request delivers audio and 120 fps, but fails
  all strict HDR color checks: its decoded primaries/transfer are SDR (6/6)
  instead of BT.2020/PQ (9/16) from the physical SDR source. Its timings cannot
  be used as a same-quality HDR comparison. No check is weakened.

### Installed candidate and capture wait investigation, 15:36–15:52 UTC

- Revision `36bff835a` is committed and pushed to the `butterpollo` remote.
  `origin` is the upstream Nonary repository; an initial push there returned
  HTTP 403 and changed nothing. Subsequent work uses the owned Butterpollo fork.
- Draft PR description/title now describe the final changes and current limits.
  Windows CI is running for the exact revision:
  https://github.com/RamazanKara/Rubylight/actions/runs/37028771427.
- Refreshed package with current documentation verified all 58 files again.
  Host bytes are unchanged from the validated candidate; package SHA-256 is
  `d196bca099e2212a599a6c554f992f02679f2b08253a0e1338490ee5765c63d7`.
- Routine installation succeeded at 15:44:23 UTC while the installed host was
  idle. The updater preserved the profile and a separate rollback backup in
  `day-work-20261002/update-36bff835a/installed-backup`. Installed host SHA-256
  matches the tested candidate; all 58 installed hashes match, service is
  running and listeners/capabilities are ready. This updater launches no
  benchmark and does not retry the rejected elevated benchmark action.
- A new normal-user, read-only DXGI wait probe changes no display settings,
  opens no window and reads back no pixels. Forty timeout samples per case on
  a separate D3D11 device: requested 1/2/4 ms waits have 7.305/11.468/10.800 ms
  means and 15.260/16.741/18.868 ms maxima. Zero-timeout acquisition costs
  0.0112 ms mean, 0.018 ms max. Arrivals are excluded from timeout distributions.
  This rules out replacing the current nonblocking worker with a short blocking
  DXGI wait on this Windows installation. It does not measure shared-device
  contention, copies, encoding or delivery. Artifact: `ddx-waits.json`.

### DDX lifecycle and normal-user motion work, 15:52–16:20 UTC

- Implemented deferred DDX release in the next candidate: release immediately
  before acquisition, and release on errors/Drop. This follows Microsoft's
  recommendation to reduce redundant desktop updates; acquisition remains
  nonblocking. Owned textures remain separate from the acquired DXGI surface.
  Switching through the CPU acquisition path invalidates the GPU pixel cache.
- The new native retention/teardown fixture passes in 0.34 seconds, across
  three duplication instances and CPU/GPU transitions. Windows' 39 ordinary
  checks, Clippy and example checks pass. Encrypted HEVC/audio compatibility
  also passes (119.974 fps, zero late intervals; host p99 3.8 ms/max 4.2 ms).
  The physical desktop's activity differs from the previous compatibility run,
  so do not attribute the lower host mean to this lifecycle change.
- Added a non-activated 128-pixel-high motion strip at the bottom of an existing
  physical output. It changes no display mode and reads source sequence/QPC
  directly from independently decoded pixels. No physical desktop frame dump
  is saved. Audio stays on the quiet test-owned virtual-speaker tone.
- The first strip launch was mistakenly started after a failed C client build
  (its compiler runtime PATH was missing). Stopped only the owned fixture tree
  and rebuilt successfully. The interrupted run is not accepted evidence.
- The next run caught a source assumption mismatch: the current Odyssey
  G93SC output is 5120×1440/120 Hz, while the earlier primary was 2560×1440.
  Diagnostics confirm two physical outputs (Odyssey and HISENSE), not a created
  virtual display. The strip fixture now diagnoses and locks the actual source
  name/dimensions, and verifies the renderer's source identity after the run.
- Corrected Rust AV1 SDR 5120×1440/120/80 Mbps requested strip run passes strict
  dimensions, independent timestamp coverage (100%), audio and permissions.
  After five seconds warmup: 3,308 unique pictures at 119.996 fps, zero repeats,
  source skips or intervals above 1.5 periods. Host mean/p95/p99/max is
  6.048/6.8/7.0/7.8 ms. Picture age is 18.468/19.222/19.678/20.744 ms. Actual
  renderer rate is 119.999 Hz. This covers partial-window native SDR motion;
  full-game/native HDR and remote scanout remain separate acceptance checks.
- Exact installed/pushed `36bff835a` Windows CI passed at
  https://github.com/RamazanKara/Rubylight/actions/runs/37028771427.

### Matched SDR motion and customer regressions, 16:20–17:03 UTC

- Active work continues from 13:29 UTC through this interval; elapsed scheduled
  time is not counted as active effort. The minimum-day continuation remains
  2026-10-03 12:30 UTC, with acceptance still required afterward.
- Exact Vibepollo `8a8c4b03` passes the same 5120×1440 SDR strip fixture at an
  actual source rate of 119.999 Hz. It delivers 119.846 unique fps, three source
  skips/late intervals; picture age mean/p95/p99/max is
  18.104/21.616/22.253/39.383 ms. The prior Rust strip run has a slightly worse
  mean but lower tails and zero late intervals. One partial-window pair is not
  proof that the product is better for the customer's HDR desktop or games.
- A source-rate-aware freshness experiment (disabled by default) passes the
  Rust strip fixture: 120.003 unique fps, no repeats/skips/late intervals;
  picture age 16.833/17.749/18.168/19.993 ms. Independent decoder mean differs
  (1.717 ms versus 2.342 ms for C++), so repeated alternating comparisons are
  required. Its arrival p99 is 9.871 ms versus 9.297 ms without the experiment;
  do not hide that tradeoff. Core fallback checks cover slower/irregular,
  restarted and overdue sources. Full-stream acceptance remains open.
- Customer reports all three codecs are still worse than Vibepollo, plus an
  Artemis HDR10 warning and occasional `invalid digit found in string`/503.
  Installed revision remains `36bff835a`; experimental scheduling is not
  installed. Recent native 1968×2184/120 HDR logs show AV1 host mean about
  6.0–6.6 ms and HEVC about 7.1–7.4 ms, with capture-age p95 around 3.5–5.2 ms.
  HEVC also has one send interval of 32.969 ms. These reports fail acceptance.
- Found a signed stream-key ID protocol defect: Android sends a signed random
  Java int, while Rust parsed only u32. Added signed/unsigned decimal handling
  that preserves all 32 IV bits and rejects out-of-range/noninteger values.
  Boundary unit check passes; encrypted end-to-end signed-ID check is pending.
- Corrected native/motion fixture guards: unauthenticated GameStream FREE is
  scoped to the requesting client and cannot prove global idleness. Guards now
  also read the installed host's connection ledger, fail closed and allow a
  quiet interval after customer activity. The candidate adds loopback-only
  counts for actual active/pending sessions and application state. Installed
  updater already checks the connection ledger; no user session is stopped.
- Confirmed GPU converter preparation costs 2415.026 ms at 1968×2184 and
  2743.044 ms at 3840×2160 (`shader-before.json`). This is a startup/reconnect
  cost, not steady frame conversion. Moving the unchanged HLSL and compiler
  optimization flags into the native Windows Rust build embeds DXBC and keeps
  the runtime compiler fallback for cross builds. Native correctness and
  after-change preparation/first-frame measurements are pending.
- HDR10 HEVC/AV1 flags are present in the installed server's current response.
  Standard codecs were published incrementally during probing. The candidate
  waits asynchronously for the complete standard codec set before answering
  serverinfo/applist. The first isolated startup sample already had HEVC HDR
  available, so the customer's warning is not yet reproduced or proven fixed.
  Exact Artemis warning clarification is optional and pending.
- Current source passes all-target Clippy. Full release tests/build are running.
  No new candidate is installed yet; no elevated benchmark retry is attempted.

### Final source validation and handoff, 17:03–17:42 UTC

- Signed Android stream-key IDs now pass an encrypted end-to-end HEVC HDR
  request: `-2147483525` preserves IV bits `0x8000007b`. Exact 1968×2184,
  BT.2020/PQ, audio and permissions pass; 1,800 total decoded frames and 2,814
  audio packets have zero decode failures. After warmup: 119.998 fps, zero late
  intervals; host mean/p95/p99/max 7.718/8.1/8.5/9.4 ms. This identifies one
  concrete cause of intermittent 503/invalid-digit failures, not every possible
  failure. Artifact: `signed-key-hevc-contract`.
- Embedded GPU shaders retain the previous HLSL and optimization flags. Native
  converter preparation falls from 2415.026 to 6.385 ms at 1968×2184, and from
  2743.044 to 1.272 ms at 3840×2160. This removes runtime compilation from the
  first frame; it is not a sustained latency result. All thirteen available
  native checks pass, including DDX retained-snapshot teardown. NVIDIA checks
  and the known failing AMD AV1 geometry check remain excluded, not passed.
  Artifacts: `shader-before.json`, `native-all-shaders`.
- All twelve PyroWave chroma/depth/framing profiles encode, and twenty-four
  encrypted/unencrypted transport cases pass the original C++ FEC and vendor
  decoder checks after shader embedding. Artifacts: `pyrowave-shaders`,
  `pyrowave-shader-transport.json`.
- Startup serverinfo waits for the complete standard codec probe. The isolated
  first response includes all standard HDR flags (`0x30301`) at about 0.593 s;
  the full probe completes in about 1.28 s instead of about 38.5 s. The prior
  first response already included HEVC HDR. The customer's Artemis warning
  remains unreproduced; obtain its exact text and test with the real client.
  Artifacts: `hdr-startup-before`, `hdr-startup-after`.
- Freshness waiting, faster fixed polling and bounded predictive polling remain
  experiments. Defaults are `capture_freshness_wait=false`,
  `capture_predictive_poll=false`, `capture_poll_interval_us=1000`. No installed
  profile was changed to enable them. Faster 100 µs fixed polling lowers mean
  picture age but produces 16 late intervals/119.386 unique fps, so it is rejected
  as a production default. Predictive polling limits fast polling to a window
  around a learned source cadence and returns to normal waits for static,
  irregular or restarted sources; its first motion result remains mixed.

Matched physical-output motion results below use a 128-pixel strip without
changing the existing display mode. Picture age includes rendering, DWM,
capture, encoding, loopback and independent software decoding; it excludes
remote display scanout. The C++ host counter excludes pre-acquisition capture
age while Rust includes it, so compare picture age and delivery together.
These runs do not establish native virtual-display or full-game acceptance.

| Case | Unique fps / late intervals | Mean / p95 / p99 / max picture age ms |
| --- | --- | --- |
| C++ AV1 SDR strip, second run | 119.765 / 3 | 16.424 / 21.132 / 22.022 / 46.623 |
| Rust AV1 SDR, embedded shaders + freshness | 119.998 / 0 | 16.173 / 16.819 / 17.286 / 17.906 |
| C++ HEVC native physical HDR, first pair | 119.798 / 4 | 15.677 / 16.057 / 17.058 / 32.182 |
| Rust HEVC physical HDR + freshness, first pair | 120.000 / 0 | 16.663 / 17.116 / 17.344 / 20.370 |
| Rust HEVC HDR + fixed 100 µs polling | 119.386 / 16 | 15.111 / 15.585 / 16.009 / 24.123 |
| C++ HEVC HDR, final pair without session API polling | 118.645 / 31 | 18.633 / 24.091 / 83.201 / 111.426 |
| Rust HEVC HDR + predictive polling, final pair | 119.355 / 14 | 15.841 / 18.115 / 19.406 / 28.169 |

The final pair's source and delivery cadence are worse than the earlier pair.
Renderer interval medians are near 119.918 Hz but do not account for every missed
vblank. Repeat under controlled CPU/GPU load and measure the producer's overall
rate before promoting any scheduling change. Do not select only the favorable
tail result or claim a whole-product victory.

At 17:49 UTC the final locked release workspace test run passes all 134 ordinary
tests (host 16, core 78, Windows 39, Vulkan 1); fifteen native tests remain
opt-in. Formatting, `git diff --check` and all-target Clippy with warnings denied
pass. Logs: `handoff-tests.log` and `handoff-clippy.log`. The current normal host
release build passes. Previous
native checks validate embedded shaders; no GPU workload was started during
the final wrap. The current source is not packaged or installed.

Active work through 17:42 covers about five hours, with the 13:20–13:29 gap
excluded; this was not a full day of active work. The user-requested wrap
records results and checkpoints code rather than continuing experiments.

## Handoff priorities and reproduction

1. Review the final source checkpoint; package/install it only when the actual
   installed host is idle. Existing package/ZIP and installed host still belong
   to `36bff835a`. Preserve configuration and pairings. Verify the negative-key
   launch and complete startup HDR flags with Artemis after installation.
2. Establish a repeated matched full-stream baseline at native 1968×2184 HDR
   120 fps, same scene, codec, bitrate and client. Diagnose capture age, send
   intervals and mouse smoothness. Keep every scheduling experiment disabled
   until both picture age and cadence improve reliably.
3. Resolve AMD AV1 decoded geometry: requested 1968×2184 becomes 1984×2186;
   1920×1080 becomes 1920×1082; 2184×1968 becomes 2240×1968. Driver FrameSize,
   alignment and explicit crop attempts have not fixed it. Do not weaken exact
   checks, falsify headers or silently change the requested picture.
4. Reproduce the HDR10 warning and display restoration error 31. NVIDIA/Intel,
   NGX, VHF feedback and full VRR/HDR client acceptance still need appropriate
   hardware. Keep the PR draft; neither latency acceptance nor release readiness
   is established.

All fixtures and raw results are in the artifact directory stated above.
`installed_state.py` reads the real connection ledger with a quiet interval;
`guard-idle.py`, `run-native.py`, `run-motion.py` and `probe-startup.py` are
normal-user runners. The physical motion runner autodetects the existing source,
stores host SHA/settings, uses a test-owned tone and saves no desktop frame dump.
`BUTTERPOLLO_TEST_POLL_SESSION_API=0` disables intrusive fixture API sampling.
The final Rust motion binary SHA-256 is
`5c6689313bff0ccb09d9d4ccaf180be4193900a4a07ec1242a1c358ad1631022`.

The pinned C++ baseline is `8a8c4b03a280ab9f567beb380110abb80f5220b8` in
`vibepollo-baseline-source`/`vibepollo-baseline-build`, with its exact modules and
startup-only test isolation patches. Its capture/codec/stream hot path is
unchanged. Do not run a globally installed C++ host for these isolated fixtures:
its startup recovery can affect the real display arrangement.

Native Rust environment: dot-source the sibling
`performance-probe/rust-env.ps1` and put `C:\msys64\ucrt64\bin` first on PATH.
Use `cargo test --locked --release --workspace`,
`cargo clippy --locked --workspace --all-targets -- -D warnings` and
`cargo fmt --all -- --check`. Native GPU checks require the idle guard.
Artifact Python is `..\test-python\Scripts\python.exe`; fixture commands and
matched settings are retained alongside their logs.

Automatic approval review rejected the elevated SYSTEM benchmark with
"blocked by policy". It was not retried through another privilege mechanism.
Subsequent tests used normal-user physical outputs. Routine idle-safe updating
was a separate accepted action. Customer streams and credentials were untouched.

Update this file with concrete changes, measurements, failures and next steps.
Record active work intervals separately from elapsed wall time and scheduled
idle gaps. A schedule running for a day does not prove a day of active effort.

## GPU reset recovery audit, 2026-10-09

Audited base `ec6b0db0` without running a stream, GPU workload, installed service,
or driver operation. Line references in this paragraph refer to that base:
`host/src/stream.rs:440,1772` joined capture/audio without deadlines;
`windows/src/display/recovery.rs:1007` likewise joined the display lease worker;
`windows/src/timing.rs:185` used `INFINITE` despite arming a deadline timer;
`host/src/state.rs:351` awaited codec readiness indefinitely on serverinfo/applist;
`windows/src/encoder.rs:583` drained FFmpeg output without an iteration bound.
These now have five-second joins, a timer wait ceiling, ten-second readiness
timeout, and a 64-packet drain limit respectively. Capture reopen checked its
deadline only after failed opens (`host/src/stream.rs:810`), so display-wait
branches could bypass it; the deadline now covers every retry branch.

Compute CPU fences already had two-second waits (`windows/src/compute.rs:92`),
including handoff/converter destruction; removal's `UINT64_MAX` is now typed
and failure paths query the device's removal reason. AMF already requested
`QueryTimeout=1` ms (`amf.rs:336`), queried once per poll (`amf.rs:1055`), and
bounded its three submission/capacity loops at 100 ms (`amf.rs:1201,1322,1407`).
There is no Present in the stream path; DDX acquisition uses timeout zero.
WGC pool errors retain HRESULTs, including across helper IPC, and its existing
one-second health check now detects a removed device even with an empty pool.

Previously generic encoder retries could reuse a dead capture device. Typed
REMOVED/RESET/HUNG/DRIVER_INTERNAL_ERROR now starts one shared recovery incident:
log the reason once through the stream-card warning, release all consumer GPU
leases, back off 150 ms, recreate capture/device and the pinned encoder, request
an IDR, and clear recovery only after encoded output. Repeated removal and
successful capture opens cannot restart the 30-second budget. A supervisor
created only on loss ends all affected clients even if vendor teardown stalls;
the terminal warning explains unavailable adapters/Code 31 and rebooting.
Healthy frames add no allocations or system calls; reason queries happen on
errors/stalls or the existing WGC health interval. No service restart is needed.

Synchronous driver calls cannot be forcibly cancelled safely in this process:
D3D11 Map (`capture.rs:892`), AMF QueryOutput and Terminate (`amf.rs:1448,1454`),
and COM/FFmpeg releases can still strand a worker until the driver or process
exits. Bounded joins detach such workers while their owned resources remain
alive. Unrelated lifetime waits remain: `display_recovery.rs:524` watches host
exit in a separate process; `crash.rs:58,66` waits for the crash reporter process;
`crash.rs:407,411,440` are crash-test synchronization. These are not GPU fences.
This change contains resets; it does not establish or fix the cause of the TDR.

GPU-free tests cover HRESULT/context classification, fence-removal sentinels,
WGC IPC/fallback, one-shot/persistent debug fault files, resource release,
recovery keyframes, fixed recovery deadlines, and client termination independent
of a blocked worker. Native DXGI adapter/display-topology tests are now opt-in
like the other hardware tests. The first ordinary workspace run unexpectedly
ran the unmarked topology test and failed with `0x80070057` in the current
environment; its assertions remain unchanged. Final validation passes: fmt
check, workspace/all-target Clippy with warnings denied, and locked workspace
tests (553 passed, 46 ignored), all through `C:\src\cargo-one.ps1` with
`CARGO_TARGET_DIR=D:\bp-build\tdr-target`. After reboot, verify normal SDR/HDR
streams, one-shot and persistent
`DXGI_ERROR_DEVICE_REMOVED[.persistent]` files in an isolated debug host's
`BUTTERPOLLO_TEST_FAULT_DIR`, the stream-card warning/IDR, and responsive console
and serverinfo throughout recovery. Actual TDR/Code 31 recovery remains a
hardware acceptance check; no forced TDR was attempted here.

## October 9 user report: physical monitor comes back on when a game loses focus

Report (the owner): streaming with the game in exclusive fullscreen, pressing
the Win key turns the physical monitor back on. Borderless and windowed games
do not. Cause, inferred from code and not yet reproduced: the exclusive layout
is applied without `SDC_SAVE_TO_DATABASE`, so the Windows display database for
"physical + virtual display" still has the physical monitor on. An
exclusive-fullscreen game losing focus (Win key, Alt+Tab, Ctrl+Alt+Del) makes
Windows apply that saved layout. The virtual display stays on, so the
heartbeat's recovery (which only starts when the virtual display is switched
off or recreated) never noticed.

- The stream heartbeat now checks the active displays every 250 ms. When a
  display the stream's layout switched off (on before the stream, off in its
  layout) has been on for 500 ms, it reapplies the layout; a refused attempt
  waits 2 s. Displays that were off before the stream, another client's
  display arriving, and the extended and primary layouts are left alone
  (`display_policy::switched_back_on`, `LayoutWatch`; `display_session.rs`
  `keep_layout`). Unit test: `a_display_the_exclusive_layout_switched_off_is_put_back_off_after_it_settles`.
- Follow-up (the owner asked for the monitor not to come on at all): an
  exclusive-layout stream on a virtual display now also saves its layout to
  the Windows display database (`SDC_SAVE_TO_DATABASE`, `Topology::save_current`),
  so the recall finds the stream's layout and switches nothing on. The entry is
  keyed by the connected displays, so only "user's displays + this virtual
  display" changes. When the last stream ends, the entry is put back (the
  user's displays on, the virtual display extended beside them) before the
  user's layout is restored. If the host died without putting it back, the
  next non-exclusive stream notices the user's displays switched off as its
  virtual display arrives and puts the entry back first. The watchdog stays
  as the fallback (a refused save, Ctrl+Alt+Del).

To confirm on the host (needs the exclusive layout, so only with the owner's
OK): one virtual display stream with the exclusive layout, then
`rust/tests/layout_recall.ps1`, which makes Windows apply its saved layout the
way a focus loss does and times how long the physical monitor stays on
(pass: never on with the saved layout; off again within 1.5 s if only the
watchdog acts; a588457 should show it staying on). After the stream, the
physical monitor must come back as before, and a following extended-layout
stream must leave it on. Then the real case: an
exclusive-fullscreen game, press Win, Alt+Tab and Ctrl+Alt+Del; the host log
shows "displays the stream layout switched off came back on; reapplying it".

## October 9 user report: audio_loss warnings on an idle host

Report (the owner, any playback device, idle PC, video perfect): the log
fills with `Audio lost on the host before sending: 4 late reads, 5.6 ms
beyond the capture buffer, 0 unsent packets (longest wait 26.6 ms, buffer
22.0 ms)`, a new line every 5 s.

Cause, from code and not yet measured on the host:

- The warning counted every sender pass that came more than the 22 ms
  capture buffer after the previous one and called the excess lost. Windows
  delivers loopback audio in whole 10 ms packets and the sender empties the
  buffer on every pass, so a third packet, and real loss, needs a wait of
  20 to 30 ms depending on phase; at 23 to 27 ms both packets usually still
  fit. With nothing playing, the endpoint delivers nothing to lose at all.
- About once a second the sender itself ran the audio route's upkeep before
  reading: keeping the streaming speakers the Windows default
  (`Route::maintain_default`) or following a moved default device
  (`Route::capture_sink`), each a fresh `MMDeviceEnumerator` and three
  `GetDefaultAudioEndpoint` calls into the Windows audio service. 4 late
  reads in 5 s averaging 1.4 ms over 22 ms fits a check taking about
  15-20 ms once a second.

Change:

- The upkeep runs on its own `audio route` thread (`RouteUpkeep`); the
  sender only reads the sink it found. The first round still runs before
  capture opens, so capture starts on the right device.
- Loss is what Windows reports: `GetBuffer`'s device position. A packet
  that starts later than the previous one ended skipped what Windows
  dropped (Chromium's WASAPI input counts loopback glitches the same way).
  `HostLoss::skipped` counts a skip only when the sender's wait explains
  it: more than half the buffer, and at least as long as the skip. A skip
  while it read on time is the endpoint falling quiet and starting again.
- The warning names the cause it saw: late reads (a busy CPU or the host,
  not bitrate) or datagrams the adapter refused (bitrate or adapter).
- Debug logs `audio sender read late or Windows skipped audio` for every
  pass more than the buffer late and every skip, with `waited_ms`,
  `skipped_ms` and `lost`. Debug builds take a one-shot `audio stall`
  fault (milliseconds) in `BUTTERPOLLO_TEST_FAULT_DIR`.

To confirm on the host (idle): `rust/release/e2e.py` streams with the
virtual-speaker tone; run 60 s HEVC 1080p60 twice on each of eef9754 and
this change with `BUTTERPOLLO_E2E_RUST_LOG=info,butterpollo=debug`. Pass:
base shows `audio_loss` lines as the owner sees; the change shows none and
few or no late passes. Positive control on a debug build: write `40` to
`<fault dir>/audio stall` every 2 s during a 30 s stream; pass: an
`audio_loss` warning with about one late read per stall and 10-20 ms lost
each. If it stays silent, Windows does not skip the loopback position on
overflow and the detector is deaf; the debug lines then show what it saw.

Host A/B, 2026-10-09 (local release builds in the rc.28 package, HEVC
1080p60, 60 s, 2 runs each): a164f72 and eef9754 both passed with the tone
decoded at 60.0 fps and 0 `audio_loss` lines; the branch logged no late
reads and no `audio_default`/`audio_device_query` warnings. The base never
showed the warning because the e2e profile sets `keep_sink_default=false`,
`auto_capture_sink=false` and a fixed virtual sink, so the once-a-second
upkeep never asked Windows anything: consistent with the upkeep being the
stall, but its cost is not measured. rc.29 shipped a164f72 (380c177).

Positive control (debug a164f72, 14 injected 40 ms stalls): 0 warnings.
Each stall logged `waited_ms=41-45 skipped_ms=0`, then 5-10 ms later a read
with `waited_ms~1 skipped_ms=18` (28 three times). The late read still finds
the packets that fit, in order; the skip shows on the next packet. rc.29's
detector only credited a skip to the read it arrived on, so it missed every
real drop. Fix: a late read stays pending for one buffer, and the first skip
in that time counts against it (`HostLoss::read`, which now also takes
whether Windows delivered anything; a late read with an empty buffer lost
nothing). `audio_probe --default-cost [rounds]` times the upkeep's Windows
calls directly (fresh enumerator, three default endpoints), read-only.

To confirm: the control again (pass: about one late read per stall, 18-28
ms lost each), and `audio_probe --default-cost 200` on the host.

Host re-test of 0863e14, 2026-10-09: all pass.

- Positive control (debug build, 16 landed 40 ms stalls): 15 late reads of
  42-45 ms, each followed by a counted skip of 18 ms (13) or 28 ms (2); one
  stall lost nothing. 6 `audio_loss` warnings 5 s apart, naming the late
  read.
- `audio_probe --default-cost 200`: avg 7.80, p50 7.73, p95 8.19, max
  11.19 ms. On this host the upkeep held the sender about 8 ms a second;
  the user's 23-27 ms reads fit a slower PC, but that is inferred.
- Normal 60 s HEVC 1080p60 e2e: passed, continuous audio, 0 `audio_loss`
  lines, 0 late or skip debug lines, 0 device warnings.

Draft for the rc.30 notes: "rc.29's audio loss warning missed real drops:
Windows reports a dropped packet on the read after the late one. It now
counts them. On the host, 15 of 16 injected 40 ms stalls were reported, each
with the 18-28 ms Windows dropped; normal streams stay silent. The device
check rc.29 moved off the audio thread took 7.8 ms on average (max 11 ms) on
the host."

## Rename to Rubylight (2026-10-09)

Ramazan picked Rubylight as the new name (analysis/rename-research.md in the
project files). Everything people see is renamed: console, tray, setup,
Start menu entry, Apps & features entry, the service's display name and the
firewall rule; setup and in-app updates replace the old Start menu entry and
firewall rule once the new ones exist. The service key (`ApolloService`),
the profile (`%ProgramData%\Butterpollo\config`), binary names, pipe, mutex
and task names, the Vulkan layer, the NVIDIA profile, the UPnP mapping
description and the virtual display label keep their names, so upgrades stay
in place and rolling back to rc.29 still works. Fresh installs default to
`C:\Program Files\Rubylight`.

The in-app updater of rc.29 and older asks
`api.github.com/repos/RamazanKara/Rubylight` and only accepts
`butterpollo-setup-<v>.exe` from that repository's download URL; after a repo
rename GitHub answers with a redirect to `api.github.com/repositories/<id>`,
which it refuses. So releases now carry the installer under both names, the
updater asks `RamazanKara/Rubylight` first and falls back to the old name, and
the repository is renamed only after rc.30 is out, so rc.29 hosts can update
to it in-app first.

Draft for the rc.30 notes: "**Butterpollo is now Rubylight.** The name nods
to Ruby, the red-haired mascot of ATI's Radeon cards. Updating keeps your
settings, paired devices, apps, service and install folder as they are; only
the names Windows shows change. The installer is now
`rubylight-setup-<version>.exe`; releases keep a `butterpollo-setup` copy so
older hosts can update in-app. Links to the old GitHub address keep
working."


## Rubylight 2.2.9 short release streams (2026-10-12)

The published 2.2.9 package at source commit
`d0bfcb5e764095250b8f0e22fd5a0cb3656d5ea6` passed all **36 short release
acceptance streams**: two runs of nine codec/output profiles on each of WGC
and Desktop Duplication (DDX). These are new measurements under the conditions
below; the earlier measurements in this work log remain historical.

- Host: Ryzen 7 5800X3D (8 cores/16 logical processors), Radeon RX 7900 XT,
  AMD driver `32.0.32015.2008`, Windows 11 Pro build `26200`.
- Capture source: the physical Odyssey G93SC at **5120x1440, 240 Hz, SDR**,
  `Bgra8`. No display-mode or HDR changes and no additional CPU/GPU load.
  Every run confirmed its requested capture backend and this source. The
  result field `comparison=matched` means that these requested and observed
  conditions matched; it does not establish a match to historical tests.
- H.264, HEVC and AV1: **2560x720 at 60 FPS, 20,000 kbps**; SDR and HDR
  outputs where listed. PyroWave: **1920x1080 at 60 FPS, 400,000 kbps**,
  record framing, with SDR/HDR and 4:2:0/4:4:4 outputs. All HDR rows encode
  the SDR source into HDR output; they are not native HDR-source measurements.
- Each independent loopback stream requested **12 seconds**, with the
  receiver's **3-second warmup** excluded from steady rate, picture-age and
  host distributions. Observed steady windows were 8.449-8.901 seconds with
  508-535 measured frames. The fixture rendered its timestamped motion strip
  at twice the requested stream rate and played the existing audio tone.
- The conventional receiver was `moonlight-client.exe`, using FFmpeg's
  software `h264`/`hevc` decoders and `libdav1d` for AV1; four decoder threads
  were requested and no hardware-decoder override was set. PyroWave used
  `moonlight-pyrowave-client.exe` and the actual PyroWave Vulkan decoder,
  bitstream `186f0393`, with synchronous **8-bit CPU-buffer readback** for the
  picture checks, including HDR output. Its decoder figure therefore includes
  that fixture's readback path; this is not a client presentation measurement.

Every numerical cell below preserves **run 1 / run 2**; no percentiles are
pooled or averaged. Rates are the steady fresh-picture FPS (`unique_fps`,
identical to `steady_fps` in every run). Picture age is the motion renderer's
QPC timestamp through capture, encoding, loopback delivery and decoded-picture
inspection on the same PC, before any client display presentation. Host
mean/p99 are the steady host-processing latency reported in received frames,
not total picture age or an encode-only timer. Decoder mean is the receiver's
**all-frame** callback mean, including warmup, unlike the steady columns.
All latency columns are milliseconds.

| Capture | Codec/output | Fresh FPS, r1 / r2 | Picture age mean, r1 / r2 | Picture age p95, r1 / r2 | Host mean, r1 / r2 | Host p99, r1 / r2 | Decoder mean, r1 / r2 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| WGC | H.264 SDR | 60.006 / 60.000 | 11.292 / 9.919 | 12.021 / 17.404 | 2.410 / 2.330 | 2.700 / 2.600 | 1.657 / 1.796 |
| WGC | HEVC SDR | 60.002 / 60.002 | 11.283 / 12.579 | 12.251 / 17.498 | 2.133 / 2.487 | 2.600 / 2.700 | 2.383 / 2.458 |
| WGC | HEVC HDR | 60.001 / 60.000 | 14.730 / 12.626 | 15.399 / 13.913 | 2.477 / 2.180 | 3.000 / 2.900 | 2.136 / 2.319 |
| WGC | AV1 SDR | 60.004 / 59.999 | 15.025 / 9.745 | 16.062 / 10.635 | 1.926 / 1.808 | 2.500 / 2.500 | 1.279 / 1.163 |
| WGC | AV1 HDR | 59.995 / 59.998 | 14.977 / 13.985 | 15.943 / 15.066 | 1.914 / 1.825 | 2.500 / 2.500 | 1.206 / 1.251 |
| WGC | PyroWave SDR 4:2:0 | 60.006 / 59.996 | 20.011 / 15.826 | 24.095 / 16.575 | 0.947 / 0.987 | 1.200 / 1.300 | 1.537 / 1.612 |
| WGC | PyroWave SDR 4:4:4 | 59.994 / 59.997 | 18.342 / 23.988 | 19.052 / 24.900 | 0.926 / 0.983 | 1.400 / 1.400 | 1.947 / 2.075 |
| WGC | PyroWave HDR 4:2:0 | 59.996 / 60.033 | 19.929 / 16.787 | 20.626 / 20.752 | 0.785 / 0.727 | 1.100 / 1.100 | 1.321 / 1.252 |
| WGC | PyroWave HDR 4:4:4 | 60.001 / 59.997 | 16.954 / 19.804 | 17.560 / 20.576 | 1.001 / 0.912 | 1.300 / 1.300 | 2.093 / 2.022 |
| DDX | H.264 SDR | 60.001 / 60.004 | 13.155 / 9.936 | 13.955 / 11.852 | 2.236 / 2.307 | 2.600 / 2.800 | 1.719 / 1.735 |
| DDX | HEVC SDR | 59.998 / 59.999 | 10.984 / 15.564 | 14.469 / 16.589 | 2.418 / 2.448 | 2.900 / 2.700 | 2.201 / 2.307 |
| DDX | HEVC HDR | 60.001 / 59.995 | 10.139 / 14.121 | 11.041 / 15.064 | 2.429 / 2.455 | 2.800 / 3.000 | 2.143 / 2.212 |
| DDX | AV1 SDR | 59.997 / 60.004 | 12.708 / 12.944 | 13.480 / 13.824 | 1.895 / 1.933 | 2.600 / 2.600 | 1.217 / 1.196 |
| DDX | AV1 HDR | 60.004 / 60.000 | 7.313 / 9.589 | 8.317 / 10.378 | 1.839 / 1.949 | 2.500 / 2.600 | 1.139 / 1.149 |
| DDX | PyroWave SDR 4:2:0 | 59.997 / 59.998 | 22.378 / 20.871 | 22.970 / 23.022 | 1.012 / 1.063 | 1.500 / 1.500 | 1.530 / 1.525 |
| DDX | PyroWave SDR 4:4:4 | 59.996 / 60.002 | 20.376 / 23.396 | 23.619 / 24.157 | 1.064 / 1.067 | 1.600 / 1.600 | 2.034 / 2.049 |
| DDX | PyroWave HDR 4:2:0 | 60.000 / 60.002 | 16.299 / 19.265 | 17.019 / 19.877 | 0.788 / 0.877 | 1.200 / 1.200 | 1.360 / 1.380 |
| DDX | PyroWave HDR 4:4:4 | 60.008 / 59.997 | 17.557 / 17.030 | 18.754 / 18.749 | 1.065 / 1.068 | 1.300 / 1.400 | 2.122 / 2.171 |

All 36 results have `passed=true`, zero decoder failures, every received frame
decoded, continuous audio tone, zero source underruns, 100% motion coverage,
and zero steady assembly intervals over 1.5 frame periods. PyroWave reported
zero partial frames. These short acceptance results do not measure remote
network behavior, real-client presentation/display latency, or long-session
stability; they do not replace the older native-HDR or real-client figures.
No competitor measurements were changed.

Provenance: the public package's `butterpollo.exe` SHA-256 is
`f38f3a0056dd64fc015af864b1bcf83c7e56bf388f97d6a6548398030665f4b7`.
The existing conventional and PyroWave receiver fixture SHA-256 values are,
respectively,
`629c0503e8816189043853ab13a6f83ea8e8236a8675a93cd779117791cbb57f` and
`72f01704a962cc6ab39e113f1acec1fcbc9fab7c57f17de798590a6702b58df0`.
The run used the unchanged release `e2e.py`, `e2e_host.py`, `e2e_result.py`
and `tests/interop.py` acceptance checks; their hashes and the motion/audio
fixture hashes are in `metadata.json`. Raw evidence is at
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\current-e2e-20261012T045200344567Z`
(started 04:52:00 UTC, ended 05:03:19 UTC), with per-run commands, capture
source, host/client logs and result JSON. The extracted 36-result set is
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\e2e-completed.json`.

### Current VRR, motion-rate and keyframe controls

The same release, host, physical SDR source and software HEVC receiver also
passed **12/12 control streams**, two runs per capture/variant. Output was
2560x720 at 60 FPS and 20,000 kbps, with 12 seconds requested and the same
3-second warmup. `VRR request` asks for VRR through the client protocol; it
is not a measurement of physical-panel presentation. `At-rate motion` renders
the timestamp strip at 60 Hz. `16 keyframes` uses that same motion rate plus
16 recovery/keyframe requests. Values remain **r1 / r2**, in ms except FPS.

| Capture | Variant | Steady received FPS | Fresh FPS | Picture age mean | Picture age p95 | Host mean | Host p99 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| WGC | VRR request | 60.000 / 60.003 | 60.000 / 60.003 | 11.866 / 17.785 | 12.709 / 18.649 | 2.056 / 2.062 | 2.300 / 2.600 |
| WGC | At-rate motion | 59.995 / 60.005 | 59.995 / 60.005 | 21.185 / 16.160 | 22.113 / 17.018 | 2.478 / 2.438 | 2.700 / 2.600 |
| WGC | 16 keyframes | 59.997 / 59.999 | 58.629 / 59.999 | 17.006 / 22.973 | 18.978 / 24.703 | 2.469 / 2.469 | 2.700 / 3.000 |
| DDX | VRR request | 59.999 / 60.000 | 59.999 / 60.000 | 14.358 / 15.599 | 17.981 / 16.226 | 2.268 / 2.062 | 2.700 / 2.300 |
| DDX | At-rate motion | 60.000 / 60.004 | 60.000 / 60.004 | 18.041 / 13.033 | 18.923 / 13.964 | 2.456 / 2.421 | 3.000 / 2.800 |
| DDX | 16 keyframes | 60.008 / 60.031 | 58.309 / 58.559 | 10.684 / 12.449 | 12.266 / 15.812 | 2.456 / 2.407 | 3.000 / 2.800 |

All four request runs received and decoded all 16 requested keyframes, and
host claim traces reported zero unchanged pictures encoded again. Request
latency below is the existing receiver's request-to-keyframe-arrival timer;
it does not include display presentation.

| Capture | Request mean, r1 / r2 | Request p95, r1 / r2 | Request max, r1 / r2 |
| --- | ---: | ---: | ---: |
| WGC | 3.780 / 4.068 | 6.514 / 6.572 | 7.061 / 8.039 |
| DDX | 3.898 / 3.581 | 6.540 / 4.899 | 7.963 / 5.657 |

Receiver picture repeats/skips were 12/12 then 0/0 for WGC and 15/15 then
13/13 for DDX. The at-rate fixture's phase can itself repeat/skip pictures;
these counts do not override the host claim trace or identify a host defect.
The request runs had 10 / 10 (WGC) and 7 / 5 (DDX) assembly intervals above
25 ms and still passed the unchanged keyframe-specific gap allowance. The
run-to-run picture-age variation is visible rather than folded into one mean.
Raw stage: `C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\current-controls-20261012T050341865344Z`.

### Current compute-conversion setting under GPU load

All **8/8** DDX HEVC HDR-output streams passed at 1920x1080, 60 FPS and
20,000 kbps, with 20 seconds requested and 3 seconds of warmup. The source
remained **physical 5120x1440 SDR at 240 Hz**, converted into HDR output.
This differs from the historical native-HDR virtual display at 120 Hz;
these settings comparisons cannot establish a before/after release change
against that older measurement. The setting is `gpu_compute_conversion`;
run 1 tested false then true, and run 2 reversed that order within each load
condition. Values are **r1 / r2**, with steady fresh FPS and ms distributions.

| Compute setting | Load | Fresh FPS | Picture age mean | Picture age p95 | Host mean | Host p99 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| false | Idle | 59.999 / 60.000 | 13.186 / 14.745 | 18.991 / 15.549 | 2.595 / 2.663 | 3.200 / 3.200 |
| true | Idle | 60.001 / 59.999 | 13.615 / 15.475 | 14.475 / 16.155 | 2.509 / 2.487 | 3.000 / 2.900 |
| false | GPU load | 60.003 / 60.000 | 21.226 / 19.821 | 25.302 / 22.031 | 4.855 / 2.612 | 10.400 / 4.000 |
| true | GPU load | 60.000 / 60.003 | 19.398 / 18.082 | 25.269 / 21.286 | 2.290 / 4.575 | 2.700 / 5.600 |

The established load command was `gpu_load.exe 45 1000 60 200`: a 45-second
2560x1440 render target, 1,000 total shader iterations split over 200 draws,
capped at 60 FPS. It started after host capability probing, stayed alive
through the stream, and exited naturally before the next case. Its own final
summary recorded **2,701 frames and 60.0 FPS in each of the four runs**;
this is load-fixture throughput, not a GPU-utilization percentage.

| Compute setting under load | Load frame p50, r1 / r2 (ms) | Load frame p95, r1 / r2 (ms) |
| --- | ---: | ---: |
| false | 9.551 / 9.794 | 10.111 / 10.458 |
| true | 9.358 / 9.871 | 9.994 / 10.032 |

The loaded host-time ordering reversed between runs: false/true means were
4.855/2.290 ms in run 1 and 2.612/4.575 ms in run 2. Picture-age tails also
varied. These two repeats support the recorded acceptance and current
conditions, not a stable general performance gain. The false/load first run
had one assembly interval above 25 ms; the other seven had none. All eight
fully decoded received frames, retained continuous audio and 100% motion
coverage, with zero decoder failures or tone-source underruns.
Raw stage: `C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\current-compute-20261012T050652545434Z`.

### Physical-source 240 FPS saturation attempts

Both final DDX HEVC SDR runs at **5120x1440, 240 FPS, 150,000 kbps** failed
the unchanged release acceptance gates. Each requested 20 seconds with the
same 3-second warmup and software receiver, using the physical 5120x1440 SDR
240 Hz source. No synthetic GPU load was added. These are complete loopback
capture/encode/transport/decode measurements; the result does not isolate a
host-only or decoder-only bottleneck and is not a real-client capacity limit.

| Run | Steady received FPS | Fresh FPS | Picture age mean (ms) | Picture age p95 (ms) | Host mean (ms) | Host p99 (ms) | Decoder all-frame mean (ms) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 107.922 | 107.507 | 391.645 | 595.534 | 8.791 | 9.500 | 8.784 |
| 2 | 106.570 | 105.799 | 393.770 | 605.422 | 8.834 | 9.500 | 8.917 |

Picture age reached 645.547 / 669.399 ms maximum; assembly-interval p99 was
44.074 / 44.533 ms and maximum 51.602 / 52.893 ms. The receiver exited 1
in both cases: steady-rate, delivery-gap and fresh-picture gates failed.
All 2,218 / 2,189 received frames decoded without decoder errors, and audio
remained continuous with no tone-source underruns; those checks do not turn
the failed 240 FPS attempt into a pass. Only final result files were included,
after both cases and the stage metadata recorded completion.
Raw stage: `C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\current-saturation-20261012T051136906993Z`.

### Supplemental capacity and 120 FPS streams

The completed supplemental stage recorded **10/12 passing checks**: both
repeated-frame capacity probes completed, and eight of ten loopback streams
passed. Both 4K120 HEVC streams failed. This remains release 2.2.9 on the
same physical 5120x1440 SDR 240 Hz source and driver recorded above. HDR
continues to mean SDR-source conversion to HDR output. Source and driver
conditions differ from historical measurements, so no historical speedup or
regression is calculated.

The additional HEVC HDR-output capacity probes used WGC, 2560x1440,
40,000 kbps and a configured 120 FPS, but ran **unpaced** on a repeated frame
for approximately eight seconds after 20 warmup frames. Each path has one
run, not a pair. The `--cpu` path still uses AMF hardware encoding and
includes CPU conversion/upload in its frame-processing call; it is not the
isolated CPU color-conversion benchmark. These figures exclude live capture,
network, decode and display. The submission-to-observed-output column is the
probe's local pipeline observation, not client picture age.

| Input path | Completed frames | Measured seconds | Encoded FPS | Encode-call mean / p99 (ms) | Submission-to-observed-output mean / p99 (ms) |
| --- | ---: | ---: | ---: | ---: | ---: |
| GPU | 3299 | 8.000453 | 412.352 | 2.425 / 4.037 | 20.741 / 21.707 |
| `--cpu` | 486 | 8.015478 | 60.633 | 16.493 / 17.552 | 32.941 / 34.837 |

The ten WGC streams each requested **20 seconds at 120 FPS**, with the same
3-second receiver warmup. Conventional HEVC/AV1 used the software decoders
and four-thread request described above; PyroWave used its Vulkan decoder
with synchronous 8-bit CPU readback. The profiles were PyroWave HDR 4:4:4 at
1080p/200,000 kbps, HEVC/AV1 HDR at 1440p/50,000 kbps, and HEVC/AV1 HDR at
4K/80,000 kbps. Values are **r1 / r2**. Age and host columns are steady
milliseconds; decoder mean includes all frames and warmup.

| Output profile | Outcome | Steady received FPS | Fresh FPS | Picture age mean | Picture age p95 | Host mean | Host p99 | Decoder mean |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| PyroWave HDR 4:4:4 1080p120 | pass / pass | 119.993 / 120.000 | 119.993 / 120.000 | 15.680 / 15.638 | 16.639 / 16.907 | 0.675 / 0.675 | 1.000 / 1.000 | 1.874 / 1.874 |
| HEVC HDR 1440p120 | pass / pass | 120.000 / 119.996 | 120.000 / 119.996 | 14.656 / 14.524 | 15.485 / 15.819 | 3.108 / 3.070 | 3.500 / 3.600 | 4.182 / 4.055 |
| AV1 HDR 1440p120 | pass / pass | 120.000 / 119.998 | 120.000 / 119.998 | 12.322 / 13.012 | 13.266 / 14.723 | 2.587 / 2.580 | 3.100 / 3.000 | 2.412 / 2.584 |
| HEVC HDR 4K120 | **fail / fail** | 97.516 / 95.425 | 97.457 / 95.425 | 548.206 / 525.010 | 887.134 / 892.375 | 5.777 / 5.716 | 6.400 / 6.300 | 10.113 / 10.235 |
| AV1 HDR 4K120 | pass / pass | 120.003 / 119.997 | 120.003 / 119.997 | 17.257 / 16.817 | 18.962 / 18.712 | 4.893 / 4.720 | 5.500 / 5.200 | 4.769 / 4.697 |

Both HEVC 4K runs exited 1 and failed receiver interoperability, steady-rate,
delivery-gap and fresh-picture gates. Their picture-age maximum was
957.890 / 967.767 ms; assembly-interval p99 was 14.967 / 17.407 ms and
maximum 45.352 / 44.905 ms, with 58 / 85 intervals over 1.5 frame periods.
Those are full-loop saturation results, not a measurement isolating the
encoder or software decoder, and not a real-client display/capacity limit.
All ten streams decoded every received frame without decoder failures and
kept continuous audio with zero source underruns. PyroWave reported zero
partial or host-replaced frames. The existing acceptance gates were unchanged;
frame completeness and audio do not erase the two failed rate/gap outcomes.
Full-precision capacity JSON and per-run commands, source checks, logs and
final results are retained in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\current-supplemental-20261012T051226067741Z`.

### Current AMF default quality

The October 12 release-source AMF quality batch completed **32/32 cases**:
16 settings for each of two clips, with **180 encoded and scored frames per
case**, starting three seconds into each clip. It uses the release 2.2.9
source commit and RX 7900 XT/driver recorded above. The exact-source
`amf_quality` probe used current defaults with no overrides, and the existing
fixture's Lanczos fill resize/crop and configured **120 FPS**. These are
single runs per clip and setting, not repeated trials or confidence intervals.
They measure file encode/decode quality and local non-IDR encode latency;
they do not include capture, transport, presentation or encoder queueing.

Quality and actual-bitrate cells below are **TMNT / GoldenEye**, referring to
`Teenage Mutant Ninja Turtles HD.mp4` and `GoldenEye XBLA.mp4`, respectively.
The two timing columns give **mean / p95 / p99 milliseconds** separately for
each clip and exclude IDR frames. Actual Mbps includes all 180 encoded
frames, including the IDR, scaled to the configured 120 FPS; it is not a
wall-clock throughput measurement. Requested and actual rates can differ
substantially for these short clips. Full precision remains in the raw JSON.

| Output / codec | Resolution | Requested Mbps | Actual Mbps (TMNT / GoldenEye) | VMAF (TMNT / GoldenEye) | PSNR-Y dB (TMNT / GoldenEye) | TMNT encode mean / p95 / p99 (ms) | GoldenEye encode mean / p95 / p99 (ms) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| SDR HEVC | 1968x2184 | 30 | 28.139 / 28.780 | 93.917 / 99.192 | 46.655 / 43.843 | 4.741 / 5.647 / 12.684 | 4.767 / 5.361 / 12.588 |
| SDR AV1 | 1968x2184 | 30 | 24.264 / 26.167 | 93.833 / 98.739 | 46.816 / 45.482 | 4.113 / 4.928 / 12.104 | 4.158 / 5.297 / 12.097 |
| SDR HEVC | 2560x1440 | 20 | 22.718 / 19.598 | 92.602 / 97.663 | 42.274 / 39.211 | 4.140 / 4.894 / 11.239 | 4.221 / 4.907 / 12.848 |
| SDR AV1 | 2560x1440 | 20 | 21.195 / 17.630 | 92.830 / 97.716 | 42.830 / 40.533 | 3.673 / 4.243 / 11.622 | 3.646 / 4.281 / 10.831 |
| SDR H264 | 2560x1440 | 20 | 22.538 / 19.285 | 90.460 / 85.368 | 41.434 / 36.310 | 4.067 / 5.005 / 11.299 | 4.093 / 4.977 / 11.411 |
| SDR HEVC | 1968x2184 | 80 | 74.412 / 80.896 | 94.905 / 99.841 | 48.324 / 48.737 | 4.783 / 5.457 / 12.460 | 4.881 / 6.019 / 12.714 |
| SDR AV1 | 1968x2184 | 80 | 39.234 / 62.251 | 94.548 / 99.387 | 47.898 / 50.089 | 4.214 / 4.747 / 13.126 | 4.203 / 5.412 / 12.259 |
| SDR HEVC | 2560x1440 | 50 | 55.221 / 51.447 | 94.301 / 99.063 | 44.289 / 42.876 | 4.157 / 4.610 / 11.074 | 4.187 / 5.160 / 11.283 |
| SDR AV1 | 2560x1440 | 50 | 40.662 / 41.199 | 94.105 / 98.782 | 44.314 / 44.092 | 3.708 / 4.466 / 11.072 | 3.666 / 4.738 / 10.948 |
| SDR H264 | 2560x1440 | 50 | 48.120 / 45.155 | 93.663 / 98.488 | 43.870 / 41.397 | 4.159 / 4.902 / 11.496 | 4.124 / 4.805 / 11.370 |
| HDR HEVC | 1968x2184 | 30 | 16.052 / 30.373 | 97.168 / 98.437 | 54.178 / 48.750 | 5.544 / 6.083 / 16.215 | 5.610 / 5.887 / 16.971 |
| HDR AV1 | 1968x2184 | 30 | 14.054 / 29.668 | 96.726 / 98.243 | 53.963 / 49.804 | 4.976 / 5.362 / 16.519 | 5.011 / 5.273 / 16.348 |
| HDR HEVC | 2560x1440 | 20 | 16.640 / 20.291 | 97.112 / 96.691 | 51.011 / 44.471 | 4.980 / 5.358 / 14.594 | 5.032 / 5.507 / 14.888 |
| HDR AV1 | 2560x1440 | 20 | 13.464 / 20.016 | 96.575 / 97.008 | 50.548 / 45.615 | 4.478 / 4.867 / 14.148 | 4.477 / 5.045 / 14.232 |
| HDR HEVC | 1968x2184 | 80 | 43.232 / 80.063 | 98.280 / 99.925 | 58.884 / 53.664 | 5.623 / 5.876 / 16.634 | 5.758 / 6.527 / 16.723 |
| HDR AV1 | 1968x2184 | 80 | 23.933 / 67.338 | 97.708 / 99.556 | 56.855 / 54.202 | 4.993 / 5.443 / 16.146 | 5.015 / 5.254 / 15.779 |
SDR references use limited-range BT.601 `yuv420p`. The HDR rows use the
existing **SDR-to-HDR 203-nit-white fixture**: the SDR clips are linearized,
converted to scRGB FP16 with white scaled by 203/80, and scored against a
BT.2020 PQ limited-range 10-bit reference. They are not native-HDR footage.
Default VMAF applied to PQ code values, and PSNR-Y in that representation,
are fixture scores rather than perceptual HDR validation; no SDR/HDR score
advantage or historical quality delta is inferred from them.

Scoring used **FFmpeg 9.0.2-full_build-www.gyan.dev**, built with GCC 16.2.0,
and its default libvmaf model. The preserved legacy graph could repeat the
last decoded picture past the intended endpoint and scored 183 pictures for
180 encoded frames in the first attempt. The corrected graph trims **both**
decoded and reference branches with `trim=end_frame=180`, aligns their time
bases, uses `shortest=1:repeatlast=0` for VMAF and PSNR, and explicitly maps
both video outputs without audio or output frame duplication. Both the SDR
and HDR validation cases passed in `amf-scoring-validation`, followed by a
fresh complete batch. Every case was checked for exactly 180 encoded frames,
180 VMAF records and 180 PSNR per-frame records. The aborted first batch
`amf-default-quality-20261012T052453Z` remains as diagnostic evidence and is
excluded from this table. Historical scalar and competitor results remain
unchanged; the scoring correction and current source/driver prevent a causal
before/after claim from these measurements.

The probe SHA-256 is
`6ae7ccb1001620d3224bd60e1b3871ebbe379a536d844d869441504ae86cc99a`;
FFmpeg SHA-256 is
`589e50b766d251afdf181dd664d40bd94407e200b019989fd7468c7d118a28d0`.
The runner hash is
`4dd91d1977bd0677766c68955bd7eceb831fb0a7fcbf44f1c7d6bfe52464fd86`.
The manifest preserves both input-clip hashes, the original SDR/HDR case CSV
and filter-script hashes, every command, per-case logs and bitstream hashes.
Raw batch: `C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\amf-default-quality-20261012T052917Z`;
scoring validation: `C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\amf-scoring-validation`.

### Current AMF quality settings comparison

The October 12 release-source supplement completed **116/116 settings
cases**, compared with the **32/32 current defaults** above. Each arm
encoded and scored exactly **180 pictures** with the same feed and scoring
commands as its saved default: 20,880 supplemental pictures and 5,760
baseline pictures, with matching VMAF and PSNR record counts. Passing means
successful encoding, decoding, scoring and applicable setting readback;
the fixture has **no minimum VMAF or PSNR threshold**.

All cases use the RX 7900 XT and release 2.2.9 source/driver recorded above,
at 120 FPS. SDR covers 1968x2184 HEVC/AV1 at 30 and 80 Mbps and 2560x1440
HEVC/AV1/H.264 at 20 and 50 Mbps. TMNT has six alternatives per case
(60 runs); GoldenEye has CBR and the opposite AQ state (20 runs). Converted
HDR covers HEVC/AV1 at 1968x2184, 30/80 Mbps, and 2560x1440, 20 Mbps:
both clips have balanced, CBR and AQ-off alternatives (36 runs).

Every range below is **alternative minus its corresponding current
default** across the stated cases. Requested bitrate is fixed; actual
bitrate is measured rather than normalized. Frame-size percentages use
the p99 of **all 180 encoded pictures, including IDR**. The default is
speed, ultra-low-latency usage, latency-constrained VBR, AQ off for H.264
and AQ on for HEVC/AV1. The four historical H.264 AQ-off alternatives
therefore become explicit **AQ-on** alternatives in this current batch.

| SDR alternative | Cases | HEVC delta VMAF | AV1 delta VMAF | H.264 delta VMAF | All-frame p99 size change, all tested codecs |
| --- | ---: | ---: | ---: | ---: | ---: |
| `amd_quality=balanced` | 10 | -0.418 to -0.080 | 0 | +0.031 to +0.143 | -2.79% to +1.95% |
| `amd_quality=quality` | 10 | -0.416 to -0.081 | -0.017 to +0.072 | +0.067 to +0.251 | -3.45% to +1.59% |
| `amd_rc=cbr` | 20 | -0.045 to +0.434 | -0.003 to +0.318 | +0.130 to +1.785 | +7.73% to +135.89% |
| `amd_vbaq=disabled` | 16 | -0.002 to +0.436 | +0.001 to +0.230 | not run; already default | -1.24% to +11.58% |
| `amd_vbaq=enabled` | 4 | not run; already default | not run; already default | -11.887 to -0.060 | -10.25% to +2.11% |
| `amd_high_motion_quality_boost=enabled` request | 10 | 0 to +0.051 | 0 to +0.036 | 0 | -0.53% to 0% |
| `amd_usage=lowlatency` | 10 | 0 | 0 | -0.007 to +0.056 | -0.08% to +1.04% |

The 10-case SDR alternatives cover TMNT only (four HEVC, four AV1 and
two H.264 cases). CBR and AQ cover both clips (eight HEVC, eight AV1 and
four H.264 cases). Each 12-case HDR alternative below has six cases per
codec, covering both clips.

| Converted-HDR alternative | Cases | HEVC delta VMAF | AV1 delta VMAF | All-frame p99 size change, both codecs |
| --- | ---: | ---: | ---: | ---: |
| `amd_quality=balanced` | 12 | -0.403 to -0.047 | -0.000045 to +0.052 | -3.25% to +4.25% |
| `amd_rc=cbr` | 12 | -0.274 to +0.719 | -0.156 to +0.362 | +9.54% to +44.50% |
| `amd_vbaq=disabled` | 12 | -0.078 to +1.086 | +0.005 to +0.482 | -4.34% to +3.62% |

**AQ conclusions:** H.264 AQ-on reduced VMAF in all four cases: at 20 Mbps,
by **1.437 TMNT / 11.887 GoldenEye** points, and at 50 Mbps by
**0.060 / 0.409**. It also reduced PSNR-Y by 0.264-2.169 dB despite using
1.49-9.32% more actual bitrate. This supports keeping H.264's current
AQ-off default for these clips. HEVC AQ-off was mixed: seven of eight
SDR scores and four of six converted-HDR scores increased, with the full
ranges above. AV1 AQ-off increased VMAF in **all 14 tested cases**, by
0.0009-0.2301 SDR and 0.0051-0.4822 converted-HDR points. These small
AV1 score gains must not be described as an unchanged score or erased by
the older findings; they also do not establish a universal visual optimum.
Converted-HDR PSNR-Y changes with AV1 AQ-off ranged from -0.141 to
+0.225 dB, so the metrics do not uniformly favour the same choice.

CBR increased all-frame p99 size in every case. Its actual-bitrate changes
were -2.31% to +17.29% SDR and +2.03% to +9.69% converted HDR; VMAF changes
were codec/content dependent. The observed larger encoded pictures do
not by themselves measure network pacing or picture age.

For timing context, the quality preset's SDR non-IDR encode-mean deltas
were **+0.705 to +0.844 ms HEVC**, **+5.595 to +6.718 ms AV1** and
**+2.002 to +2.067 ms H.264**; corresponding p99 deltas were
+0.495 to +0.843, +4.823 to +6.747 and +1.663 to +1.808 ms.
Low-latency usage had observed mean deltas of +5.489 to +8.245 ms and
p99 deltas of +5.299 to +9.088 ms across codecs, with the small or zero
score changes above. CBR SDR means ranged from -0.123 to +0.169 ms,
but one AV1 p99 delta reached +7.379 ms. Full per-case mean/p95/p99
values and deltas are retained rather than interpreting that tail as a
repeatable setting effect. Baselines ran before the supplement, without
randomization or interleaved controls: **none of these timing deltas is a
matched performance comparison**.

Effective-setting logs verify **106/116 requested properties**. The other
**10 HMQB requests have no property readback in this release**; none
logged an AMF-setting-unavailable warning. Their completed scores describe
the requested configuration, but neither completion nor absent warnings
prove HMQB applied. The summary preserves the exact property, requested
value, observed value, full effective-settings line and warnings for every
case. There were no setting mismatches, driver-unavailable warnings or
interrupted attempts in this batch.

These remain file encode/decode measurements, excluding capture,
transport and display. The converted-HDR input is SDR with **203-nit
white**, scored in PQ code values with the default VMAF model; it is not
native-HDR perceptual validation. All current arms start at **3 seconds**,
including GoldenEye HDR, whose historical fixture started at 5 seconds.
Together with the corrected 180-picture scorer, this prevents a matched
historical quality delta. The October 8 tables and competitor results
remain unchanged.

Full-precision results for all 116 alternatives and their exact 32 control
matches are indexed in `work\amf-settings-quality-summary.json`, generated
by `work\summarize_amf_settings_quality.py`. Raw evidence is under
`work\measurements\amf-settings-quality-20261012T093917Z`; the controls
are `work\measurements\amf-default-quality-20261012T052917Z`.
The summary verifies manifest/run/result agreement, control hashes,
effective-setting log hashes and both 180-record score files. It retains
all commands through the manifests, clip/probe/FFmpeg/fixture identities,
per-case recorded evidence hashes, actual bitrate, VMAF, PSNR-Y, non-IDR
encode timings and both IDR-inclusive and non-IDR frame-size p99 values.

### Current default pacing in complete loopback streams

The published 2.2.9 host passed **4/4** default-pacing streams on October 12,
07:43–07:45 CEST: two runs each of **2560x720 HEVC SDR at 60 FPS / 60,000
kbps** and **2560x720 PyroWave SDR at 120 FPS / 800,000 kbps**. These use the
same host, driver and physical **5120x1440 SDR / 240 Hz** source recorded
above, with WGC, the fixture's minimum-FPS target matching the stream rate,
and no pacing overrides or synthetic load. Runs alternate HEVC then PyroWave
in each repeat. Each requests 30 seconds; the receiver excludes three seconds
of warmup from steady rate, picture-age and host distributions. Observed
steady windows were 26.748–26.917 seconds.

These are independent **loopback** capture/encode/transport/decode streams.
HEVC uses the FFmpeg software decoder with four requested decoder threads.
PyroWave uses its Vulkan decoder, bitstream `186f0393`, encrypted record
framing and synchronous **8-bit CPU-buffer readback**. Picture age includes
the timestamped source picture through decoding/readback on the same PC;
it excludes physical network transit and client display. Decoder means below
include warmup and all frames, unlike the steady picture-age/host metrics.
They do not establish real-client or Wi-Fi latency, native-HDR correctness,
or a causal change from a historical pacing build.

| Profile / run | Decoded / received frames | Steady received FPS | Fresh FPS | Picture age mean / p95 / p99 / max (ms) | Host mean / p99 (ms) | Decoder mean (ms) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| HEVC / 1 | 1792 / 1792 | 60.007 | 60.007 | 13.206 / 15.298 / 15.740 / 22.480 | 2.396 / 2.900 | 2.145 |
| HEVC / 2 | 1787 / 1787 | 60.006 | 60.006 | 13.084 / 18.426 / 18.976 / 19.668 | 2.392 / 2.600 | 2.751 |
| PyroWave / 1 | 3593 / 3593 | 120.002 | 119.742 | 14.342 / 15.262 / 17.120 / 22.974 | 0.864 / 1.300 | 1.197 |
| PyroWave / 2 | 3593 / 3593 | 119.999 | 119.999 | 14.423 / 15.248 / 17.669 / 22.704 | 0.865 / 1.200 | 1.230 |

All four passed the unchanged release/interop acceptance gates, with 100%
motion coverage, no video decode failures, continuous audio and zero
tone-source underruns. No steady assembly interval exceeded 1.5 frame
periods. Assembly-interval p99 / maximum were 20.957 / 21.384 ms and
20.359 / 21.370 ms for HEVC, then 10.357 / 12.005 ms and
9.874 / 11.535 ms for PyroWave. Both PyroWave runs reported zero partial
frames and zero host-replaced frames; the first run's lower fresh-picture
rate remains visible in the table. This short check does not establish
long-session stability or a display-presentation bound.

The package and both receiver hashes match the 36-stream release batch
above. The task-local runner `work\run_default_pacing_streams.py` has SHA-256
`46ecc91de62b8814191cf16fae6a799fd229502fffcfba35734ed5be932ac240`.
It uses the unmodified release `e2e.py` and `tests/interop.py` with the modes,
bitrates and 30-second duration above. Exact per-case commands, source checks,
host/client logs and final `result.json` files are retained alongside the
stage manifest at
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\current-default-pacing-streams-20261012T054303698009Z`.

### Audio under load and 80 ms outage recovery

The published 2.2.9 host completed **11** serial 30-second loopback tests on
October 12, 07:50–07:57 CEST. **Audio was continuous in all 11**, with nonzero
received packets and tone blocks, peaks of 0.012907–0.012912, and **zero
tone-source underruns**. Full-stream acceptance was **6 passed / 5 failed**;
audio continuity does not turn a video failure into a pass. All received
video frames decoded without reported decoder failures, with 100% motion
coverage. This is the independent FFmpeg software receiver with four decoder
threads, the existing virtual-speaker tone and moving strip, and WGC on the
same **5120x1440 SDR / 240 Hz** physical source. It does not measure a laptop,
Wi-Fi, client presentation or native HDR.

The eight audio load cases use **2560x720 SDR at 60 FPS / 20,000 kbps**,
with `minimum_fps_target=60` and default pacing. CPU load means **16 separate
normal-priority Python busy processes**, each bounded to 180 seconds and
stopped after stream teardown. The historical `batch.py` is unavailable, so
this substitute does not establish an exact historical CPU-load comparison.
GPU load is the archived 2560x1440 fixture, `gpu_load.exe 45 1000 0 200`:
45 seconds, 1,000 total iterations, uncapped and 200 draws. Load starts only
after encoder capability detection; every requested CPU/GPU load remained
alive through stream teardown. GPU loads then completed naturally with exit
status zero. Combined means both loads, not an equal GPU workload rate.

There is one run per condition. Steady rates and picture age exclude three
seconds of warmup; audio packet/tone-block counts cover the full receiver
run. Picture age is measured only for received, decoded pictures and does
not include the time spent displaying an old picture across a lost-frame
gap. The gap measurements below retain that separate failure mode.

| Case | Full-stream result | Audio packets / tone blocks | Steady received / fresh FPS | Picture age p95 / max (ms) |
| --- | --- | ---: | ---: | ---: |
| H264 / idle | passed | 5831 / 10862 | 60.000 / 60.000 | 14.884 / 16.849 |
| H264 / cpu | passed | 5709 / 10618 | 60.055 / 59.753 | 19.546 / 39.749 |
| H264 / gpu | failed | 5831 / 10862 | 60.003 / 48.859 | 67.954 / 96.313 |
| H264 / combined | passed | 5728 / 10656 | 59.993 / 57.981 | 26.802 / 69.849 |
| HEVC / idle | passed | 5811 / 10822 | 60.000 / 60.000 | 15.218 / 16.890 |
| HEVC / cpu | passed | 5731 / 10662 | 60.021 / 59.490 | 20.980 / 51.216 |
| HEVC / gpu | failed | 5805 / 10810 | 60.002 / 47.508 | 70.125 / 113.416 |
| HEVC / combined | passed | 5747 / 10694 | 59.957 / 58.441 | 27.749 / 47.315 |
| H264 / 80 ms outage | failed | 5833 / 10866 | 113.671 / 113.671 | 14.008 / 32.977 |
| HEVC / 80 ms outage | failed | 5827 / 10854 | 113.080 / 113.080 | 15.309 / 39.876 |
| AV1 / 80 ms outage | failed | 5809 / 10818 | 113.573 / 113.573 | 13.392 / 31.367 |

Both GPU-only cases failed **“moving pictures were missing or repeated too
often”**: fresh-picture rates of 48.859 and 47.508 FPS were below the
unchanged 90%-of-requested-rate gate, despite receiving about 60 FPS. Their
assembly intervals exceeded 1.5 frame periods 4 and 8 times respectively;
this did not independently fail the existing gap gate. The passing combined
cases are not evidence that CPU contention improves the stream: their GPU
load completed substantially fewer frames. Actual whole-load throughput and
frame timings were:

| Stream / load | Load frames | Load fps | Load frame p50 / p95 (ms) |
| --- | ---: | ---: | ---: |
| H264 / GPU only | 7699 | 171.1 | 5.714 / 6.535 |
| H264 / combined | 3001 | 66.7 | 10.299 / 37.254 |
| HEVC / GPU only | 7723 | 171.6 | 5.694 / 6.498 |
| HEVC / combined | 3051 | 67.8 | 11.384 / 35.441 |

The three recovery cases use **2560x1440 SDR at 120 FPS / 80,000 kbps**,
`minimum_fps_target=120` and `pacing_max_bitrate_kbps=160000`, without the
CPU/GPU load. The existing `BUTTERPOLLO_TEST_SEND_OUTAGE=1500:80:air`
fixture simulates an 80 ms video-send outage every 1.5 seconds;
`BUTTERPOLLO_TEST_RFI=1` advertises reference invalidation. This is an
injected loopback failure, not measured radio loss. No normal acceptance
gate was relaxed or given a forced-outage allowance. All three retained the
same failures: **receiver interoperability failed**, **steady frame rate is
outside 97–103% of the requested rate**, and **frame delivery has excessive
gaps**. Their steady long-interval counts were 33 / 36 / 64 for H264 / HEVC /
AV1, with maximum intervals of 97.080 / 107.072 / 95.806 ms.

`recovery_gaps.py` analyzes the complete timing CSV, including warmup. A
stall requires both missing wire-frame indices and an arrival gap exceeding
twice the median frame period. Thus its stall count differs from the
receiver's steady intervals-over-1.5-period counter. Gap length includes the
outage and recovery; missing frames are gaps in wire-frame indices, not UDP
packet-loss counts. Frame sizes below use 1,024 bytes per KiB.

| Codec | Decoded / received | Stalls | Gap mean / p95 / max (ms) | Missing frames | Stalls ended by keyframe | Median / recovery-mean frame size (KiB) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| H264 | 3395 / 3395 | 19 | 95.7 / 97.0 / 97.1 | 190 | 19 | 14.8 / 148.8 |
| HEVC | 3369 / 3369 | 19 | 105.2 / 106.9 / 107.1 | 208 | 19 | 12.8 / 184.4 |
| AV1 | 3381 / 3381 | 19 | 88.0 / 88.1 / 95.8 | 191 | 0 | 7.4 / 21.0 |

H264 and HEVC ended all 19 detected stalls with a keyframe. AV1 ended none
with a keyframe; its client log records post-invalidation P-frames. That
observed recovery behavior does not make AV1's failed normal full-stream
result a pass or establish a historical recovery improvement.

Raw evidence is retained in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\audio-starvation-recovery-20261012T055019Z`.
It includes all 11 `run.json` and unmodified e2e `result.json` files, source
checks, host/client/tone/load logs, three recovery CSVs and their gap reports,
and the manifest with exact commands and fixture hashes. The package and
receiver hashes match the current release stream batch above. Reproduce with
the task-local `work\run_audio_starvation.py`, SHA-256
`c93062ba316e84421c4aff2ca76184c5eef4f68597d4b2e3dee04ffe9c9fd0a8`.

### IDR requests on the physical desktop without imposed motion

All **12/12** IDR/interoperability checks passed on the published 2.2.9 host
on October 12, 07:58–08:04 CEST: two repeats of H264, HEVC and AV1 through
each of WGC and DDX. The same physical **5120x1440 SDR / 240 Hz** desktop
remained live, with **no imposed motion fixture and no guarantee that its
content was frozen**. Applications could redraw during a run. These are not
matched reproductions of an earlier static scene or measurements of native
HDR, laptop/Wi-Fi delivery or display presentation.

Each run requests 25 seconds, **60 FPS / 20,000 kbps SDR**, with
`minimum_fps_target=20`. H264/HEVC output is **1920x1080**; AV1 output is
**2560x1440**. The independent FFmpeg software receiver requests four decoder
threads, excludes two seconds of warmup, and requests **10 IDRs** at the
fixture's default 1.5-second spacing. Every run logged **10 requested, 10
sent, 10 corresponding keyframes observed and 10 decoded**, with no video
decode failures. The arrival statistics below measure request to complete
keyframe enqueue at the receiver, excluding keyframes already in flight;
they are not request-to-display latency. The existing ten-sample percentile
calculation is retained.

Pass here means the unchanged `interop.py` and receiver completed their
IDR/decode/audio checks with `INTEROPERABILITY PASS`. The task-local worker
does not run the release `e2e_result.py` motion/freshness evaluator and does
not request minimum-FPS or moving-picture checks for this source. Observed
cadence is reported without relabelling this as a complete moving-stream
acceptance test. Steady windows were 22.800–22.900 seconds; frame and audio
packet totals cover the full receiver run. Long intervals exceed 1.5 frame
periods, and their counts and tails remain visible below.

| Capture / codec / repeat | Decoded / received frames | Steady FPS | IDR arrival mean / p95 / max (ms) | Long intervals | Arrival interval p99 / max (ms) | Audio packets |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| WGC / H264 / r1 | 1491 / 1491 | 60.000 | 3.795 / 5.541 / 5.581 | 4 | 18.194 / 28.128 | 4823 |
| WGC / HEVC / r1 | 1492 / 1492 | 60.001 | 3.903 / 6.426 / 7.307 | 3 | 21.357 / 27.679 | 4827 |
| WGC / AV1 / r1 | 1492 / 1492 | 59.999 | 3.978 / 5.277 / 6.091 | 2 | 18.054 / 27.913 | 4823 |
| DDX / H264 / r1 | 1496 / 1496 | 60.001 | 4.181 / 6.043 / 6.804 | 5 | 17.918 / 28.545 | 4829 |
| DDX / HEVC / r1 | 1491 / 1491 | 59.999 | 3.960 / 5.828 / 6.714 | 4 | 17.813 / 28.426 | 4807 |
| DDX / AV1 / r1 | 1496 / 1496 | 60.000 | 3.731 / 3.658 / 5.643 | 4 | 17.774 / 29.186 | 4827 |
| WGC / H264 / r2 | 1492 / 1492 | 59.999 | 3.993 / 5.467 / 6.183 | 5 | 17.852 / 28.377 | 4827 |
| WGC / HEVC / r2 | 1493 / 1493 | 60.000 | 3.369 / 3.244 / 6.414 | 4 | 17.599 / 28.389 | 4827 |
| WGC / AV1 / r2 | 1492 / 1492 | 59.999 | 4.072 / 5.349 / 6.246 | 3 | 20.242 / 33.106 | 4827 |
| DDX / H264 / r2 | 1495 / 1495 | 60.000 | 3.945 / 5.696 / 7.121 | 4 | 17.868 / 28.812 | 4823 |
| DDX / HEVC / r2 | 1497 / 1497 | 60.000 | 3.484 / 3.409 / 6.840 | 4 | 17.889 / 28.669 | 4831 |
| DDX / AV1 / r2 | 1496 / 1496 | 60.000 | 4.246 / 6.623 / 7.639 | 5 | 20.515 / 28.770 | 4827 |

All 12 runs recorded continuous audio, 8,814–8,862 measured tone blocks and
zero tone-source underruns. No synthetic CPU or GPU load was added. The
capture backends and SDR source dimensions were confirmed in host logs.
Neither fresh-picture rate nor picture age is established by this fixture.

Raw evidence is retained in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\remaining-static-idr-20261012T055813241409Z`:
per-case run/source/result records, host/client/tone logs, receiver timing
and `timing.csv.idr.csv` request/arrival/decode records, plus stage metadata,
hardware and the 12-entry summary. Package and conventional receiver hashes
match the current release batch above. Reproduce with
`work\run_remaining_streams.py --stages static-idr`; the runner SHA-256 is
`2660304689116b0e6f7f0e1abb2d6336bee86033ce818693f3e5672731ec554f`.

### Ninety-second control and 20 ms outage streams

Eight published-2.2.9 runs completed on October 12, **08:04–08:16 CEST**:
two repeats of HEVC and AV1, each with a control and an injected-outage run.
All **four controls passed**; all **four outage runs failed** the unchanged
normal-stream gate with **“frame delivery has excessive gaps”**. Every run
used **2560x1440 SDR at 120 FPS / 50,000 kbps**, WGC on the same physical
5120x1440 SDR / 240 Hz source, `minimum_fps_target=120`, default pacing and
the moving-strip/audio-tone fixtures. No synthetic CPU or GPU load was added.

Both controls and outage runs advertise reference invalidation through
`BUTTERPOLLO_TEST_RFI=1`. Only outage runs set
`BUTTERPOLLO_TEST_SEND_OUTAGE=1500:20:air`, simulating **20 ms of video-send
loss every 1.5 seconds**. The independent FFmpeg software receiver uses four
decoder threads over **loopback**. This does not measure real Wi-Fi loss,
laptop hardware decoding, client display or native HDR. The 90-second runs
have 86.797–86.842-second steady windows after three seconds of warmup.

All received frames decoded with zero reported decoder failures, 100% motion
coverage, continuous audio and zero tone-source underruns. Received and fresh
FPS are identical in these results. Frame/audio totals cover the entire
receiver run; cadence statistics are steady-state. Long intervals exceed
1.5 frame periods. The outage rates remain within the normal 97–103% FPS
gate; their gaps fail independently. No forced-outage exception was added.

| Codec / condition / repeat | Full-stream result | Decoded / received frames | Steady received = fresh FPS | Long intervals | Arrival interval p99 / max (ms) | Audio packets / tone blocks |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| HEVC / control / r1 | passed | 10780 / 10780 | 120.000 | 0 | 9.576 / 11.965 | 17823 / 34846 |
| HEVC / outage / r1 | failed | 10564 / 10564 | 117.549 | 116 | 13.544 / 40.318 | 17831 / 34862 |
| AV1 / control / r1 | passed | 10782 / 10782 | 120.001 | 0 | 8.904 / 10.334 | 17829 / 34858 |
| AV1 / outage / r1 | failed | 10600 / 10600 | 117.999 | 157 | 13.161 / 38.329 | 17825 / 34850 |
| HEVC / control / r2 | passed | 10784 / 10784 | 120.000 | 0 | 9.500 / 10.580 | 17831 / 34862 |
| HEVC / outage / r2 | failed | 10570 / 10570 | 117.659 | 116 | 13.496 / 41.199 | 17821 / 34842 |
| AV1 / control / r2 | passed | 10781 / 10781 | 120.002 | 0 | 11.195 / 11.936 | 17829 / 34858 |
| AV1 / outage / r2 | failed | 10567 / 10567 | 117.608 | 139 | 12.815 / 38.419 | 17825 / 34850 |

The separate `recovery_gaps.py` reports use the complete timing CSV,
including warmup, and the same missing-wire-frame plus two-median-period
stall definition as the 80 ms check above. All controls recorded **zero
detected stalls**. Every outage run recorded **59 stalls**; gap duration
includes loss and recovery. Missing frames below count gaps in wire-frame
indices, not missing UDP packets, and frame sizes are KiB (1,024 bytes).

| Codec / repeat | Stalls | Gap mean / p95 / max (ms) | Missing frames | Stalls ended by keyframe | Median / recovery-mean frame size (KiB) |
| --- | ---: | ---: | ---: | ---: | ---: |
| HEVC / r1 | 59 | 36.8 / 40.0 / 40.3 | 219 | 59 | 14.8 / 142.9 |
| HEVC / r2 | 59 | 35.6 / 39.8 / 41.2 | 211 | 59 | 17.7 / 138.2 |
| AV1 / r1 | 59 | 29.1 / 29.7 / 38.3 | 180 | 0 | 12.3 / 13.3 |
| AV1 / r2 | 59 | 34.5 / 38.0 / 38.4 | 214 | 0 | 13.2 / 19.4 |

HEVC ended every detected stall with a keyframe. AV1 ended none with a
keyframe, with 59 post-invalidation P-frames logged in each repeat. This
shows recovery in this injected-loss fixture while preserving all four
normal-stream failures; it does not establish a historical recovery gain
or real-network performance.

Raw evidence is retained in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\remaining-loss-20261012T060405396678Z`:
all eight run/source/e2e result records, host/client/tone logs, timing CSVs
and recovery reports, plus stage metadata, hardware and summary. The host,
receiver and runner hashes match the current static-IDR batch above.
Reproduce with `work\run_remaining_streams.py --stages loss`; each recorded
command retains the 90-second duration, 50,000 kbps bitrate and outage
environment where applicable.

### Two-minute decoded-audio continuity and video windows

Both **120-second** published-2.2.9 streams passed the unmodified release
e2e gates on October 12, **08:16–08:21 CEST**. HEVC uses **2560x720 SDR /
60 FPS / 20,000 kbps**; PyroWave uses **1920x1080 HDR-output 4:4:4 / 60 FPS /
400,000 kbps** with encrypted record framing. Both use WGC on the same
physical **5120x1440 SDR / 240 Hz** source, `minimum_fps_target=60`, default
pacing and no added CPU/GPU load. PyroWave's HDR output is converted from
SDR, and its Vulkan receiver uses synchronous **8-bit readback**; this is
not native-HDR capture or full-precision HDR pixel validation. HEVC uses
the independent FFmpeg software receiver with four decoder threads.

These are loopback decode/continuity measurements, excluding real-network
transit, laptop performance and audio/display presentation. All received
frames decoded, with zero decode failures, 100% motion coverage, continuous
audio and zero tone-source underruns. PyroWave reported 7,195 complete HDR
records, zero partial frames and zero host-replaced frames. Steady received
and fresh FPS were identical. Frame/audio totals cover the full receiver
run; picture age and cadence exclude three seconds of warmup, leaving
116.796 seconds for HEVC and 116.867 seconds for PyroWave.

| Profile | Decoded / received frames | Steady received = fresh FPS | Picture age mean / p95 / p99 / max (ms) | Audio packets / tone blocks |
| --- | ---: | ---: | ---: | ---: |
| HEVC SDR | 7190 / 7190 | 60.002 | 14.240 / 17.236 / 17.857 / 25.826 | 23819 / 46838 |
| PyroWave HDR output 4:4:4 | 7195 / 7195 | 60.000 | 18.444 / 19.356 / 19.846 / 20.626 | 23855 / 46910 |

Neither run had a steady assembly interval over 1.5 frame periods. The
whole-steady interval p99 / maximum was **20.411 / 22.127 ms** for HEVC and
**19.991 / 20.934 ms** for PyroWave.

Offline analysis uses the unchanged `soak_result.py` `audio_windows` and
`video_windows` functions. Each audio timeline has **12 nominal ten-second
windows, including its partial final window**; all **24/24** were continuous
under the existing tone RMS check. In each case, the first eleven audio
windows contain 2,000–2,002 packets and 480,000–480,480 decoded samples per
channel at 48 kHz. Final-window counts are reported separately below, not
normalized to a full ten seconds. The callback-gap range is the minimum to
maximum of the 12 per-window maximum gaps, not a percentile or speaker-output
gap measurement.

| Profile | Continuous audio windows | Window maximum callback-gap range (ms) | Partial final audio packets / decoded samples per channel |
| --- | ---: | ---: | ---: |
| HEVC SDR | 12 / 12 | 19.026–19.841 | 1807 / 433680 |
| PyroWave HDR output 4:4:4 | 12 / 12 | 10.818–11.474 | 1843 / 442320 |

The video analyzer independently creates 12 windows per case after its
three-second warmup. Each has 100% picture-age coverage and zero assembly
gaps exceeding **two frame periods (33.333 ms)**, a separate criterion from
the e2e 1.5-period counter. Ranges below include the partial final window and
retain per-window statistics; they are not pooled percentiles.

| Profile | Window fresh-FPS range | Window decoder-mean range (ms) | Window picture-age p99 range (ms) | Partial final video frames / span (s) |
| --- | ---: | ---: | ---: | ---: |
| HEVC SDR | 59.982–60.014 | 2.193–2.425 | 14.053–18.274 | 403 / 6.700 |
| PyroWave HDR output 4:4:4 | 59.983–60.016 | 2.096–2.184 | 19.429–20.159 | 406 / 6.750 |

Audio timestamps are receiver QPC after Opus decoding; these video
timestamps are QPC at decode-callback start. Their windows have independent
origins and must not be aligned by window index. No source-audio timestamp,
RTP clock mapping or synchronized audiovisual marker is present. These
results establish decoded-tone continuity and video cadence only, **not
lip-sync, A/V clock drift or glass-to-glass timing**.

Raw evidence is retained in
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\remaining-audio-20261012T061645540435Z`;
the offline windows and evidence hashes are in the task-local
`work\audio-windows-summary.json`. Host/receiver identities match the current
release batch above. Reproduce the measurements with
`work\run_remaining_streams.py --stages audio`, then analyze the retained
CSVs with `work\summarize_audio_windows.py`. The analyzer wrapper SHA-256 is
`82c08a2231bfe774c7e5748000aa25ba3cb337dcf2330d76d01cfe9b58662ad6`;
the unchanged `soak_result.py` SHA-256 is
`ac98ec52e34580dfbd83b9b8cee1d590a3ea81829afa0e3f9ce327ee6a127289`.

### Short soak, switching, reconnects and WGC-helper recovery

The valid published-2.2.9 short soak ran on October 12, **08:27–08:36 CEST**,
for **564.789 seconds** overall. Of **67 cases, 64 passed and three failed**:
all three idle steady streams, eight codec/mode switches, 52 launch/resume
cycles and the WGC-helper recovery passed; all three loaded steady streams
failed freshness checks. The overall soak result remains **failed**. This
roughly nine-minute matrix contains short individual sessions and does not
establish multi-hour stability.

It uses the same released host and receiver identities above, an isolated
loopback host with `wgc_user_helper=true`, the moving-strip/tone fixtures and
the physical **5120x1440 SDR / 240 Hz** display snapshot. Conventional codecs
use the FFmpeg software receiver; PyroWave uses its Vulkan receiver and
8-bit readback. HDR switch cases convert the SDR source and do not establish
native-HDR correctness. The stock soak calls its receiver evaluator without
the e2e source-comparison arguments, so receiver records retain
`comparison=unverified`; the display snapshot and host logs are separate
source evidence, not a matched historical comparison.

The six steady cases each request **25 seconds, 1920x1080 SDR and 20,000
kbps**. Both 120 FPS cases request VRR; minimum-FPS targets match each
stream rate. After three seconds of warmup, steady windows span
21.498–21.550 seconds. There is one run per condition, in the order below.
All six decoded every received frame, had continuous audio with zero
tone-source underruns, and recorded 100% picture-age coverage.
Decoder means include all frames; rates and picture-age statistics exclude
warmup.

| Profile / condition | Result | Decoded / received | Steady received / fresh FPS | Picture age mean / p95 / p99 / max (ms) | Decoder mean (ms) |
| --- | --- | ---: | ---: | ---: | ---: |
| HEVC 60 FPS / idle | passed | 1476 / 1476 | 59.999 / 59.999 | 14.740 / 19.822 / 20.418 / 21.294 | 3.929 |
| HEVC 60 FPS / load | failed | 1475 / 1475 | 59.997 / 42.304 | 54.182 / 81.100 / 93.041 / 122.201 | 3.837 |
| AV1 60 FPS / load | failed | 1474 / 1474 | 60.006 / 41.492 | 51.745 / 80.482 / 94.876 / 108.285 | 2.336 |
| AV1 60 FPS / idle | passed | 1474 / 1474 | 60.000 / 60.000 | 11.365 / 13.658 / 14.004 / 18.934 | 2.102 |
| HEVC 120 FPS / idle | passed | 2949 / 2949 | 120.004 / 120.004 | 15.213 / 16.200 / 16.539 / 21.553 | 2.890 |
| HEVC 120 FPS / load | failed | 2962 / 2962 | 120.630 / 49.349 | 52.510 / 81.078 / 94.680 / 117.468 | 2.831 |

Each loaded case retains both failures: **“moving pictures were missing or
repeated too often”** and **“a video window lost fresh pictures”**. Its two
full video windows delivered 42.202 / 42.207 fresh FPS for HEVC at 60 FPS,
42.009 / 40.567 for AV1 at 60 FPS, and 48.430 / 49.307 for HEVC at 120 FPS, below the
existing 90%-of-requested-rate threshold. These were not reclassified as
decoder-limited cases. The stock load command is `gpu_load.exe 55 1000 0 200`
(uncapped, 200 draws); the soak stops it after the 25-second stream. All
three load logs are empty, so **completed load throughput is unavailable**;
measurements from other suites are not substituted.

All eight ten-second switches passed in this order: H264, HEVC, AV1,
PyroWave, HEVC HDR output, 1280x720 HEVC, PyroWave HDR-output 4:4:4, then
H264. Other outputs are 1920x1080, all at 60 FPS; requested bitrate is
20,000 kbps for conventional codecs and 400,000 kbps for PyroWave. They
decoded 571–581 frames each, sustained 59.983–60.035 fresh FPS, and retained
continuous audio with zero source underruns.

All **52 three-second HEVC 1080p60 churn cases** passed their existing
short-session checks: 26 launches alternating with 26 resumes on one host.
They decoded all 153–156 received frames per case, received 405–413 audio
packets, and reported 59.969–60.139 fresh FPS after 0.5-second warmup. These
checks require decoded video and audible audio, but deliberately omit the
tone-continuity test; `audio_continuous` is null, so these are not 52 further
continuity measurements. No teardown failures or leftover-device/helper
failures were reported. The **26 disconnect-to-next-resume-request intervals
were 0.504–0.534 seconds**. This is request-dispatch timing, not time to the
first decoded frame. The existing quick-reconnect check passed.

Resource growth compares 26 settled churn snapshots after each resume,
discards the first five, then compares the first/last five medians of the
remaining 21 samples. The host working-set, handle and thread checks passed:

| Host resource | First median | Last median | Delta | Allowed growth |
| --- | ---: | ---: | ---: | ---: |
| Working set, bytes | 155529216 | 153108480 | -2420736 | 67108864 |
| Handles | 391 | 389 | -2 | 16 |
| Threads | 40 | 39 | -1 | 4 |

GPU memory is **outside that resource-growth gate**. For the same host and
adapter, the first/last settled churn snapshots (`churn-02` / `churn-52`)
recorded the per-process `DedicatedUsage` counter at
**4,365,221,888 / 17,118,527,488 bytes**, an increase of 12,753,305,600 bytes.
These original readings remain preserved as counter observations. The
[follow-up below](#gpu-memory-counter-follow-up-october-12) reproduced the
growing counter without sustained adapter-wide VRAM growth, identifying
the large staircase as a reporting artifact. It must not be presented as
12.75 GB of leaked VRAM.

The final 20-second HEVC 1080p60 case terminates only the isolated host's
owned WGC helper at **8.442 seconds**. The host logged `capture_recovery`;
the existing bounded-recovery evaluator reported **recovered**, observed by
the end of a valid video window **11.121 seconds after injection**. That
post-injection window spans 6.550 seconds, with 60.002 fresh FPS and 100%
picture-age coverage. This observation time is not the outage duration:
the receiver's maximum assembly gap was **496.039 ms**. It decoded all
1,148 frames and reported continuous tone with zero source underruns; both
audio windows were continuous. The fault case uses its recovery predicate,
not normal steady-stream cadence acceptance. The retained host log also
contains an `audio_stopped` warning near teardown.

Three debug-only fault cases remain excluded from this published-binary
measurement: `DXGI_ERROR_ACCESS_LOST`, `encoder failure` and
`encoder failure.persistent`. The stock fixture also skips controller/VHF
streaming because its receiver has no controller sender, and secure-desktop
tests because it does not change Windows desktop/security state. The
excluded debug access-lost case is not coverage for the secure desktop.

An earlier **152.134-second invalid fixture attempt** is preserved at
`work\measurements\remaining-soak-short-20261012T062104519442Z` and excluded
from product pass/failure counts. Both directly launched receivers exited
before `main` with `-1073741515` because the DLL search path omitted
`C:\msys64\ucrt64\bin`, which `interop.py` normally supplies. The retained
no-argument check `work\soak-dll-startup-check.json` reproduces that exit,
then obtains exit 2 with **“session URL required”** from the same binaries
after adding that directory; no stream was launched by the check. The
task-local rerun prepends the directory **only to the receiver environment**.
The released host, source matrix and evaluators remain unchanged apart from
excluding the three unsupported debug faults. The initial startup/cascading
resume errors are not 67 product failures.

Valid raw evidence is retained at
`C:\Users\ramaz\Documents\Codex\2026-10-12\wait\work\measurements\remaining-soak-short-20261012T062718020881Z\soak-short\soak\20261012T062721-072655Z`.
It includes all 67 case records and timelines, host/client/tone logs,
settled resource snapshots and the final `result.json`; stage metadata and
`release-limitations.json` record the exact package and fixture adaptation.
Reproduce with `work\run_remaining_streams.py --stages soak-short`; this
runner version has SHA-256
`c9e16755be55449132c47952982e1cbcad768cc0f173e12ae79ed04a57b57563`.

#### GPU-memory counter follow-up (October 12)

A focused diagnostic on the same published **2.2.9** host, RX 7900 XT,
driver and physical desktop ran at **14:06–14:10 UTC**. It reused the first
30 HEVC 1920x1080p60 reconnect cases from the existing soak fixture,
alternating launch/resume with three-second streams through the WGC helper.
All **30/30 cases** and the existing CPU resource-growth checks passed.
The eight-second pairing stream accounts for the additional disconnect:
the log contains **31 completed disconnects** and no stuck-worker teardown
warnings. This diagnostic adds separate adapter-wide counters and a
30-second final idle period; it does not repeat the mixed-codec or GPU-load
parts of the original soak.

The original sampler reads
`Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory`, filtered
by host PID. The follow-up also reads `GPUAdapterMemory` for the same
RX 7900 XT adapter (`luid_0x00000000_0x00016702_phys_0`). Each snapshot has
one matching process instance; the increase is not a duplicated-row sum
or a running total calculated by the fixture. GB below means decimal GB.

| Observation | Process dedicated counter (GB) | Process GPU commitment (bytes) | Adapter dedicated usage (GB) |
| --- | ---: | ---: | ---: |
| Before test host starts | not running | not running | 2.664 |
| After 30 reconnects, host still running | 7.910 | 4,096 | 2.786 |
| After 30 seconds idle, host still running | 7.910 | 4,096 | 2.659 |
| After test host exits | not running | not running | 2.659 |

All 15 settled reconnect snapshots report **4,096 bytes** of process GPU
commitment. Adapter dedicated usage returns from **2,664,321,024 bytes**
before launch to **2,659,160,064 bytes** by 15 seconds idle, remaining there
at 30 seconds and after host exit. Adapter commitment changes from
**3,152,412,672 to 3,152,408,576 bytes** on exit, a decrease of exactly
**4,096 bytes**. There is no corresponding 7.91 GB of adapter consumption
to release. The original soak also returned process commitment to 4,096
bytes in all 41 settled snapshots while its dedicated counter accumulated.

The reproduced staircase is a **per-process counter/accounting artifact**;
no sustained adapter VRAM growth was observed in this HEVC/WGC reconnect
test. Microsoft documents [false growth in GPU Process Memory counters](https://learn.microsoft.com/en-us/troubleshoot/windows-client/performance/gpu-process-memory-counters-report-wrong-value)
during resource recreation. That Windows 10 report is supporting context,
not confirmation of the underlying Windows/driver defect on this Windows
11 build. These results resolve the large counter-growth warning; they do
not establish every codec, capture path or longer session as leak-free.

Raw evidence is retained under
`work\leak-analysis\repro-20261012T140648Z`, including the original
`independent-counters.jsonl`, fixture metadata and the soak results in
`soak\20261012T140650-275038Z`. The counter timeline has SHA-256
`f2e69f58ec3195e03d5f626e9631c4ae8a43267771d8be53f5d18d48b03f2521`;
the soak `result.json` has
`90e3306e5f0fc5d398884003baeb428bb6058ddf7bcff6c64992a052c1224f03`.
`work\leak-analysis\reproduction-summary.json` indexes the comparison and
hashes; `soak-counter-audit.json` retains the original 92-snapshot audit.
The task-local diagnostic is `work\leak-analysis\reproduce_counter.py`,
with its sampler and released-host identity pinned in the metadata.
Original benchmark outcomes and competitor measurements remain unchanged.

### Real Wi-Fi hardware-receiver streams (October 12)

The laptop connection was restored, and **all 20 independent-receiver
streams passed their raw receiver gates**: 12 HDR-output conversion cases
and eight SDR control/recovery cases. These completed between **07:28 and
07:58 UTC**. They use the published 2.2.9 host above, not the historical
rc.24 host. The [two completed stock Moonlight sessions](#stock-moonlight-620-over-wi-fi-october-12)
are reported separately with their original cleanup-check failures.

The source remains the physical **5120x1440 SDR display at 240 Hz** on the
RX 7900 XT. A timestamped motion strip renders at twice the requested stream
rate; GPU compute conversion is enabled and `minimum_fps_target` matches
the requested stream rate. HDR outputs therefore measure **SDR-to-HDR
conversion**, not native-HDR capture or HDR pixel fidelity. The receiver is
on the actual Lenovo IdeaPad (model 83D3), Radeon **780M**, driver
`32.0.31041.1004`, Windows build `26200`, using its RZ616 Wi-Fi 6E adapter
with a reported **2.4 Gbps link**. Link rate is not measured payload
throughput. The laptop was on AC power with the Balanced power plan.

The task-owned independent receiver uses **D3D11VA/D3D11 hardware decoding**
and reads back an eight-row barcode strip from NV12/P010 surfaces. Its
executable SHA-256 is
`68a6e78ece95d7ac48719da5772bafb179f0bdb06ea3ee827e15d469ffd50a67`;
the retained fixture source reference is
`e5520ac0:rust/tests/moonlight_client.c`, not an attestation of a reproducible
build. This is distinct from installed Moonlight **6.2.0** and does not
measure client display presentation. Receiver metadata retains its actual
executable identity, GPU/driver, and CSV/log hashes for each run.

| Family | Output and requested rate | Requested bitrate | Duration per run | Runs / raw gates |
| --- | --- | ---: | ---: | ---: |
| HEVC and AV1 HDR output | 1920x1080, 60 FPS | 20 Mbps | 30 s | 4 / 4 passed |
| HEVC and AV1 HDR output | 2560x1440, 120 FPS | 50 Mbps | 30 s | 4 / 4 passed |
| HEVC and AV1 HDR output | 1968x2184, 120 FPS | 80 Mbps | 30 s | 4 / 4 passed |
| HEVC and AV1 SDR controls | 2560x1440, 120 FPS | 50 Mbps | 90 s | 4 / 4 passed |
| HEVC and AV1 SDR with sender outage | 2560x1440, 120 FPS | 50 Mbps | 90 s | 4 / 4 passed |

Each condition has two separate runs, with the second pass in reverse
condition order. The qualification HEVC HDR 1080p60 run is run 1 of that
condition, not an extra 21st stream. Recovery cases request reference-frame
invalidation support; outage cases use the existing **`1500:20:air`** sender
fixture (20 ms every 1,500 ms). Steady receiver gates exclude three seconds
of warmup and request a minimum received rate of 97% of the stream rate.
These are the pinned receiver's audio, picture, motion and FPS gates; they
are not the local software-receiver release evaluator or a matched
historical comparison.

Final analysis accepted **19 of 20 timing records**. Host render QPC is
mapped onto the laptop clock using **2,777 of 13,883 replies** (the
lowest-RTT 20%, rounded up) and one affine fit. Both QPC frequencies are
10 MHz. The fitted drift is **16.929 ppm**, selected maximum RTT **1.631 ms**
and maximum absolute fit residual **0.362 ms**. Selected half-RTT plus
maximum fit residual gives a **1.177 ms sample-based diagnostic**, not a
confidence interval or a proven error bound; network asymmetry remains
unknown. Every accepted source/arrival timestamp lies inside the selected
calibration span; no clock extrapolation is used.

For this hardware fixture, `arrival_ms` estimates completed frame
assembly/enqueue time in the laptop QPC epoch, excluding decoder-queue
waiting. Render-to-received timing therefore excludes decoder work and
display scanout. The separately reported decoder callback includes barcode
readback; adding it to arrival age would omit decoder-queue wait. The CSV's
local `picture_age_ms` is invalid across machines and is not used.

The table retains each repeat separately; no percentiles are pooled or
averaged. All latency columns are milliseconds. Distributions use the first
received copy of each unique readable source picture after three seconds
of warmup; fresh FPS counts unique-picture transitions over the complete
post-warmup arrival window. Host processing is reported by received frames,
not an encode-only timer. Decoder mean below uses the same selected pictures,
unlike the receiver log's all-frame callback mean.

| Codec/output and condition | Output / FPS | Run | Fresh FPS | Render-to-received mean / p95 / p99 | Host mean / p99 | Decoder mean |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| HEVC HDR output | 1920x1080 / 60 | 1 | 60.018 | 14.123 / 15.295 / 16.834 | 2.454 / 3.000 | 3.463 |
| HEVC HDR output | 1920x1080 / 60 | 2 | 60.001 | 9.025 / 15.818 / 17.103 | 2.368 / 2.700 | 3.311 |
| AV1 HDR output | 1920x1080 / 60 | 1 | 59.997 | 10.697 / 12.103 / 13.128 | 1.977 / 2.600 | 3.452 |
| AV1 HDR output | 1920x1080 / 60 | 2 | unavailable | unavailable | unavailable | unavailable |
| HEVC HDR output | 2560x1440 / 120 | 1 | 120.009 | 11.764 / 13.138 / 16.199 | 3.115 / 3.600 | 4.608 |
| HEVC HDR output | 2560x1440 / 120 | 2 | 119.996 | 11.503 / 13.594 / 16.078 | 3.060 / 3.600 | 4.668 |
| AV1 HDR output | 2560x1440 / 120 | 1 | 120.003 | 11.544 / 14.531 / 15.945 | 2.660 / 3.100 | 4.700 |
| AV1 HDR output | 2560x1440 / 120 | 2 | 119.988 | 10.463 / 11.486 / 12.621 | 2.581 / 3.000 | 4.599 |
| HEVC HDR output | 1968x2184 / 120 | 1 | 119.998 | 12.057 / 14.152 / 16.333 | 3.523 / 4.100 | 5.269 |
| HEVC HDR output | 1968x2184 / 120 | 2 | 119.999 | 15.202 / 39.992 / 70.001 | 3.594 / 3.900 | 5.265 |
| AV1 HDR output | 1968x2184 / 120 | 1 | 119.961 | 11.069 / 11.972 / 13.559 | 2.914 / 3.300 | 5.228 |
| AV1 HDR output | 1968x2184 / 120 | 2 | 119.998 | 11.024 / 11.901 / 13.369 | 2.947 / 3.400 | 5.278 |
| HEVC SDR control | 2560x1440 / 120 | 1 | 119.999 | 12.170 / 13.765 / 47.960 | 3.086 / 3.600 | 4.616 |
| HEVC SDR control | 2560x1440 / 120 | 2 | 119.999 | 11.469 / 12.794 / 14.495 | 3.095 / 3.600 | 4.587 |
| HEVC SDR outage | 2560x1440 / 120 | 1 | 117.878 | 11.234 / 12.576 / 14.954 | 3.112 / 3.600 | 4.602 |
| HEVC SDR outage | 2560x1440 / 120 | 2 | 117.931 | 11.458 / 12.707 / 14.285 | 3.076 / 3.600 | 4.593 |
| AV1 SDR control | 2560x1440 / 120 | 1 | 119.998 | 10.622 / 11.540 / 12.842 | 2.661 / 3.100 | 4.621 |
| AV1 SDR control | 2560x1440 / 120 | 2 | 120.003 | 10.852 / 12.225 / 14.890 | 2.603 / 3.000 | 4.652 |
| AV1 SDR outage | 2560x1440 / 120 | 1 | 117.771 | 10.849 / 12.250 / 15.181 | 2.644 / 3.100 | 4.652 |
| AV1 SDR outage | 2560x1440 / 120 | 2 | 117.722 | 10.772 / 11.835 / 15.791 | 2.607 / 3.100 | 4.623 |

AV1 HDR 1080p60 run 2 passed its receiver gates (1,795/1,795 decoded frames,
zero decode failures, continuous tone and **60.002 raw steady FPS**), but
three consecutive decoded barcode ID/QPC pairs did not match the exact
source ledger. Its entire calibrated timing record remains unavailable;
no samples were filtered or substituted. The raw log's 100% readable-barcode
coverage does not prove timestamp identity. The mismatch diagnostic is
retained as `source-pair-mismatch-details.json` beside that case. All 20
runs reported continuous tone and zero decoder failures. The HEVC portrait
run 2 tail (70.001 ms p99) and the HEVC SDR control run 1 tail (47.960 ms
p99) are retained; ordinary receiver acceptance does not imply low latency
tails, and these runs do not establish a causal change from the older
source/driver fixture.

Recovery analysis uses the existing `recovery_gaps.py` criterion over the
whole CSV, including warmup: missing wire-frame numbers **and** an adjacent
arrival gap exceeding twice the median period. Stall length includes the
outage and recovery; a stall ending in `frame_type=1` ends in a keyframe.
RFI/IDR counters come separately from observed host sessions. The pinned
**11-column CSV has no frame-byte field**, so median/recovery-frame byte
sizes are unavailable and must not be reported as zero.

| SDR codec / condition | Run | Detected stalls | Gap mean / p95 / max (ms) | Missing wire frames in stalls | Keyframe-ended stalls | Last observed RFI / IDR / send-loss counters |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| HEVC control | 1 | 0 | not applicable | 0 | 0 | 0 / 1 / 0 |
| HEVC control | 2 | 0 | not applicable | 0 | 0 | 0 / 1 / 0 |
| HEVC outage | 1 | 59 | 33.783 / 40.574 / 41.651 | 192 | 59 | 117 / 60 / 0 |
| HEVC outage | 2 | 59 | 32.281 / 39.524 / 40.444 | 183 | 59 | 118 / 60 / 0 |
| AV1 control | 1 | 0 | not applicable | 0 | 0 | 0 / 1 / 0 |
| AV1 control | 2 | 0 | not applicable | 0 | 0 | 0 / 1 / 0 |
| AV1 outage | 1 | 59 | 32.523 / 38.470 / 40.425 | 198 | 0 | 117 / 1 / 0 |
| AV1 outage | 2 | 58 | 33.761 / 39.543 / 40.636 | 203 | 0 | 117 / 1 / 0 |

These host counters are cumulative at the last observed session sample,
not guaranteed teardown totals, and are not summed across polling samples.
They retain the difference between reference feedback and IDR requests;
the number of RFI requests is not a count of independently recovered
outages. All detected HEVC outage stalls ended in keyframes; none of the
detected AV1 outage stalls did. Missing-wire counts apply only to detected
stalls, not every lost packet or frame on the network.

Raw evidence is retained in task-local
`work\measurements\wifi-20261012T072600-000003Z` (qualification) and
`work\measurements\wifi-20261012T073200-000004Z` (remaining 19 cases).
Each includes its plan, release/fixture hashes, isolated host lifecycle,
source motion ledger and original laptop evidence archive. Final timing
analysis is retained in `work\wifi-summary.json`, using the stopped
collector's `wifi-clock-20261012T070500\clock-pings.csv` and stopped
responder's `laptop-20261012T075905-clock-final.json`. Per-case
`calibrated-timing-final.json` files retain accepted results or the explicit
validation failure and their evidence hashes. Historical Wi-Fi and
competitor measurements above remain unchanged.

### Native virtual-HDR headline streams (October 12)

The published **2.2.9** host passed **28/28** native virtual-HDR headline
streams between **08:26 and 08:52 UTC**. All 28 also passed exact source
ID/QPC membership, timestamp ordering and CSV/statistic accounting checks;
no mismatching samples were filtered. These are two repeats of seven
conditions through each of DDX and WGC. The host is the same Ryzen 7
5800X3D / RX 7900 XT, driver `32.0.32015.2008`, Windows build 26200 used
above. Published host SHA-256:
`f38f3a0056dd64fc015af864b1bcf83c7e56bf388f97d6a6548398030665f4b7`;
release source `d0bfcb5e764095250b8f0e22fd5a0cb3656d5ea6`.

The captured source is an extended **native HDR virtual display at 120 Hz**,
with the requested output dimensions and confirmed `RgbaF16` capture.
The physical **5120x1440 SDR / 240 Hz** Odyssey G93SC remains active.
The historical October 8 setup instead had a **2560x1440 / 120 Hz** physical
display. These current results do not establish a matched improvement over
the earlier host or competitor measurements. The renderer runs for 40 seconds;
the independent receiver requests 35 seconds and excludes the first five
seconds from steady statistics. Host frame pacing remains default, with
1,000 us capture polling and predictive/freshness polling disabled.

The local independent receiver uses D3D11VA hardware decoding on the
RX 7900 XT, with the same pinned executable as the Wi-Fi fixture
(`68a6e78ece95d7ac48719da5772bafb179f0bdb06ea3ee827e15d469ffd50a67`;
retained source reference `e5520ac0:rust/tests/moonlight_client.c`, not a
reproducible-build attestation). **Picture age here is the renderer's QPC
timestamp to the local decoded-barcode measurement**, including hardware
decode and eight-row barcode readback. It excludes client presentation and
physical display scanout. Both processes use the same 10 MHz QPC clock, so
there is no Wi-Fi clock-fit adjustment. This endpoint differs from the
Wi-Fi table's render-to-received endpoint. The hardware CSV's separate
`arrival_ms` estimates frame assembly/enqueue, **not decode start**;
`arrival_ms + decode_ms` omits decoder-queue waiting and is not a
decode-completion timestamp. Its 11 columns contain no frame-byte field.

Each row below retains one repeat, with no pooled percentiles. Latencies
are milliseconds. Picture-age statistics use readable post-warmup decoded
pictures; host and callback means use post-warmup received frames. Fresh FPS
counts unique-picture transitions over the post-warmup arrival window.
Ordinary acceptance is the unchanged fixture's independent stream gate,
separate from the exact-source timing validation.

| Capture | Codec | Load | Output / FPS / Mbps | Run | Fresh FPS | Picture age mean / p95 / p99 | Host mean / p99 | Decoder callback mean |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| DDX | HEVC HDR | idle | 1968x2184 / 120 / 80 | 1 | 120.001 | 10.213 / 10.721 / 11.138 | 3.247 / 3.600 | 2.198 |
| DDX | HEVC HDR | idle | 1968x2184 / 120 / 80 | 2 | 119.985 | 9.086 / 11.014 / 13.191 | 3.344 / 4.000 | 2.319 |
| DDX | AV1 HDR | idle | 1968x2184 / 120 / 80 | 1 | 119.998 | 8.962 / 9.586 / 10.005 | 3.038 / 3.500 | 2.321 |
| DDX | AV1 HDR | idle | 1968x2184 / 120 / 80 | 2 | 119.999 | 12.042 / 12.926 / 13.214 | 3.026 / 3.500 | 2.263 |
| DDX | HEVC HDR | idle | 2560x1440 / 120 / 50 | 1 | 120.000 | 9.716 / 10.231 / 10.543 | 2.984 / 3.600 | 1.954 |
| DDX | HEVC HDR | idle | 2560x1440 / 120 / 50 | 2 | 119.999 | 9.053 / 9.553 / 9.844 | 2.956 / 3.500 | 2.069 |
| DDX | HEVC HDR | idle | 1920x1080 / 60 / 20 | 1 | 60.000 | 9.218 / 9.683 / 9.883 | 2.179 / 2.600 | 1.638 |
| DDX | HEVC HDR | idle | 1920x1080 / 60 / 20 | 2 | 59.999 | 10.966 / 11.431 / 11.643 | 2.099 / 2.600 | 1.549 |
| DDX | HEVC HDR | load | 1968x2184 / 120 / 80 | 1 | 119.980 | 15.261 / 17.579 / 18.074 | 3.450 / 4.400 | 3.796 |
| DDX | HEVC HDR | load | 1968x2184 / 120 / 80 | 2 | 120.000 | 13.418 / 14.473 / 18.227 | 3.244 / 4.100 | 2.753 |
| DDX | AV1 HDR | load | 1968x2184 / 120 / 80 | 1 | 119.997 | 14.895 / 19.201 / 19.886 | 5.145 / 8.600 | 2.671 |
| DDX | AV1 HDR | load | 1968x2184 / 120 / 80 | 2 | 119.971 | 12.395 / 16.789 / 18.016 | 5.352 / 8.600 | 2.439 |
| DDX | HEVC HDR | load | 1920x1080 / 60 / 20 | 1 | 59.999 | 11.661 / 21.844 / 22.270 | 2.192 / 3.000 | 1.446 |
| DDX | HEVC HDR | load | 1920x1080 / 60 / 20 | 2 | 60.006 | 9.616 / 14.687 / 14.890 | 2.281 / 2.800 | 1.581 |
| WGC | HEVC HDR | idle | 1968x2184 / 120 / 80 | 1 | 119.965 | 12.950 / 15.449 / 19.332 | 3.260 / 3.700 | 2.356 |
| WGC | HEVC HDR | idle | 1968x2184 / 120 / 80 | 2 | 120.000 | 11.526 / 12.018 / 12.488 | 3.264 / 3.600 | 2.406 |
| WGC | AV1 HDR | idle | 1968x2184 / 120 / 80 | 1 | 120.001 | 9.566 / 10.309 / 13.414 | 3.040 / 3.500 | 2.513 |
| WGC | AV1 HDR | idle | 1968x2184 / 120 / 80 | 2 | 120.015 | 11.577 / 13.240 / 13.627 | 3.035 / 3.400 | 2.482 |
| WGC | HEVC HDR | idle | 2560x1440 / 120 / 50 | 1 | 119.998 | 12.135 / 12.824 / 13.054 | 2.954 / 3.200 | 2.129 |
| WGC | HEVC HDR | idle | 2560x1440 / 120 / 50 | 2 | 120.004 | 10.447 / 11.168 / 11.351 | 2.990 / 3.400 | 2.124 |
| WGC | HEVC HDR | idle | 1920x1080 / 60 / 20 | 1 | 60.000 | 8.826 / 9.475 / 9.845 | 2.270 / 2.600 | 1.685 |
| WGC | HEVC HDR | idle | 1920x1080 / 60 / 20 | 2 | 60.001 | 9.892 / 10.379 / 10.611 | 2.275 / 2.600 | 1.652 |
| WGC | HEVC HDR | load | 1968x2184 / 120 / 80 | 1 | 119.926 | 15.450 / 18.708 / 19.151 | 3.368 / 4.000 | 4.083 |
| WGC | HEVC HDR | load | 1968x2184 / 120 / 80 | 2 | 119.964 | 18.043 / 20.070 / 20.349 | 3.397 / 4.000 | 4.624 |
| WGC | AV1 HDR | load | 1968x2184 / 120 / 80 | 1 | 119.760 | 17.212 / 19.436 / 24.054 | 3.033 / 3.600 | 4.474 |
| WGC | AV1 HDR | load | 1968x2184 / 120 / 80 | 2 | 119.976 | 17.428 / 19.520 / 19.812 | 3.053 / 3.500 | 4.647 |
| WGC | HEVC HDR | load | 1920x1080 / 60 / 20 | 1 | 59.998 | 13.258 / 13.965 / 19.763 | 2.289 / 2.500 | 1.490 |
| WGC | HEVC HDR | load | 1920x1080 / 60 / 20 | 2 | 60.000 | 10.618 / 11.069 / 11.212 | 2.453 / 2.800 | 2.449 |

Loaded cases use the established `gpu_load 45 1000 60 200` fixture:
a 2560x1440 render target, 1,000 total iterations across 200 draws, capped
at 60 FPS for 45 seconds. Every one of the 12 load processes completed
naturally with **2,701 frames / 60.000 FPS**; observed per-run frame-time
p50 ranged **5.419–6.179 ms**, and p95 **10.022–10.344 ms**. This is a
specified synthetic workload, not a claim of maximum GPU saturation.
Variation between repeats, including the 24.054 ms WGC AV1 loaded p99,
is retained. Two repeats do not establish confidence intervals or causal
improvements over a different display/driver setup.

Raw streams and controls are under `work\measurements\headline-ddx` and
`work\measurements\headline-wgc`, with batch suffixes
`20261012t082617-467057z` and `20261012t083911-689666z`. Each retains the
source ledger, original receiver CSV/log, capture integrity and before/after
active-monitor snapshots. The snapshots verified original active monitor
identities, layout, mode, refresh and HDR restoration; they are not a full
QDC topology snapshot. Final source validation, per-run results and hashes
are in `work\native-summary.json`. Earlier launcher and pairing setup
failures are retained separately, not counted as product stream failures.
Stock Moonlight validation remains separate from this instrumented receiver.

### Native HDR pixel roundtrip (October 12)

All **4/4** WGC native-HDR pixel checks passed: two HEVC and two AV1 runs,
**1280x720 / 60 FPS / 40 Mbps**, 35 seconds each, using a 120 Hz extended
virtual HDR source. These differ from the historical 22-second / 60 Hz
source fixture. This check uses the software-decoding independent receiver
(`629c0503e8816189043853ab13a6f83ea8e8236a8675a93cd779117791cbb57f`),
not the hardware receiver's reduced barcode strip. The existing
`colour_check.py` black, white, luma, chroma, contrast and saturation gates
all passed; each run retained 16 native-HDR observations. Code errors below
are in limited-range ten-bit code values; slope and saturation are ratios.

| Codec | Run | Black Y | White Y | Luma mean / mean absolute / p99 absolute error | Chroma mean absolute error | Contrast slope | Saturation ratio |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| HEVC HDR | 1 | 64.01 | 509.00 | -0.073 / 0.423 / 1.88 | 0.408 | 0.9996 | 1.0059 |
| HEVC HDR | 2 | 64.00 | 509.00 | -0.068 / 0.392 / 1.74 | 0.397 | 0.9993 | 1.0061 |
| AV1 HDR | 1 | 64.00 | 509.00 | 0.000 / 0.311 / 1.36 | 0.354 | 0.9990 | 1.0051 |
| AV1 HDR | 2 | 64.00 | 509.00 | -0.046 / 0.372 / 1.72 | 0.574 | 0.9978 | 0.9917 |

Expected black/white Y are **64.00 / 509.08**. Raw evidence and unchanged
gates are retained under `work\measurements\hdr-pixels`, with executable,
fixture and colour-check hashes in `work\native-summary.json`. Active
monitor state was restored in all four cases. This validates the native
HDR capture/conversion/codec pixel path; it is not physical-panel
calibration, full-game perceptual validation or a display-latency test.

### SDR VRR capture-only measurements (October 12)

The current-source `capture_phase_probe` attempted **32 runs across eight
requested mode groups**, with no added GPU load. **20 runs in five groups**
matched the requested Windows display mode and source rate; all 20 captured
every presented picture in the retained window. **12 runs in three groups
have unavailable requested-mode timings** because Windows selected a
different active refresh rate. All raw attempts remain recorded.

Each group orders WGC helper / DDX / DDX / WGC helper, ten seconds each,
excluding the first second and final 250 ms. The source is SDR BGRA8.
This measures renderer Present to a distinct owned capture texture after
GPU copy, with no encoder, receiver, network or scanout. The executable is
built against exact release source `d0bfcb5e`, SHA-256
`792ff4e1ed774815a460e7266a1851b42c3ecc01fa2daacf78ce2976400d77e2`;
`work\vrr-fixtures\build.json` pins the source and runtime. It is a
source-built component fixture, not a downloaded release executable or
the unavailable private HDR/RTSS variant.

Each paired value below preserves the first / second occurrence of that
backend, not an average. Latency columns are milliseconds; all listed runs
have **100%** picture coverage. The 116 FPS runs contain 1,015 retained
presented/captured pictures each, and 120 FPS runs 1,050 each.

| Output / active Hz / source FPS | Capture | Observed source FPS, run 1 / 2 | Present-to-copy mean, run 1 / 2 | p95, run 1 / 2 | p99, run 1 / 2 |
| --- | --- | ---: | ---: | ---: | ---: |
| 1920x1080 / 1000 / 116 | WGC helper | 115.997 / 115.996 | 8.108 / 8.240 | 10.374 / 10.592 | 10.893 / 11.337 |
| 1920x1080 / 1000 / 116 | DDX | 116.015 / 115.995 | 8.014 / 7.965 | 10.203 / 10.161 | 10.624 / 10.734 |
| 1920x1080 / 1000 / 120 | WGC helper | 120.003 / 120.000 | 8.630 / 8.826 | 9.919 / 10.132 | 10.411 / 10.387 |
| 1920x1080 / 1000 / 120 | DDX | 119.998 / 120.001 | 8.606 / 8.564 | 9.503 / 9.491 | 10.050 / 10.029 |
| 3840x2160 / 1000 / 116 | WGC helper | 115.996 / 115.999 | 8.001 / 8.234 | 10.486 / 10.615 | 11.040 / 11.219 |
| 3840x2160 / 1000 / 116 | DDX | 116.002 / 115.997 | 7.675 / 7.632 | 9.823 / 10.047 | 10.338 / 10.603 |
| 3840x2160 / 1000 / 120 | WGC helper | 120.004 / 119.998 | 8.709 / 8.508 | 9.874 / 9.708 | 10.767 / 10.105 |
| 3840x2160 / 1000 / 120 | DDX | 120.000 / 119.998 | 7.891 / 8.019 | 8.996 / 8.979 | 9.178 / 9.207 |
| 1920x1080 / 232 / 116 | WGC helper | 116.006 / 116.000 | 6.497 / 6.484 | 8.490 / 8.438 | 8.918 / 9.071 |
| 1920x1080 / 232 / 116 | DDX | 115.998 / 115.999 | 6.277 / 6.229 | 8.333 / 8.300 | 8.780 / 8.886 |

| Requested output / Hz / source FPS | Observed active Hz | Attempts | Requested-mode timing |
| --- | ---: | ---: | --- |
| 1920x1080 / 240 / 120 | 120 | 4 | unavailable |
| 3840x2160 / 232 / 116 | 928 | 4 | unavailable |
| 3840x2160 / 240 / 120 | 960 | 4 | unavailable |

The virtual-display API requested lease rates, but this retained probe did
not force Windows active-mode selection. The mismatched groups were not
retried or relabelled as requested-mode passes. Their captured-picture
coverage does not override the mode mismatch. Observed active Hz is a
Windows virtual-display mode, not measured physical-panel refresh.
All eight groups restored the recorded active-monitor state. Raw group
directories are `work\measurements\vrr-capture-*` from `085538473358Z`
through `090444056183Z` on October 12; the exact eight paths, all 32 raw
reports and hashes are indexed by `work\native-summary.json`. The two
remaining 4K groups ran separately after the first batch stopped on its
1080p240 mismatch. The historical private native-HDR capture-only and
RTSS fixture sources remain unavailable; no replacement experiment or
matched historical improvement is claimed.

### Stock Moonlight 6.2.0 over Wi-Fi (October 12)

Two published **2.2.9** sessions used the stock Moonlight Qt **6.2.0**
client on the Radeon 780M laptop over its RZ616 Wi-Fi connection. The
stream used HEVC SDR 1920x1080 at 60 FPS, requested 80 Mbps,
WGC, `gpu_compute_conversion=false`, and `minimum_fps_target=20`.
The encoder negotiated 62,988 kbps with 20% FEC. Each client ran for
165 seconds with hardware decoding, V-sync and frame pacing disabled.
The physical source was 5120x1440 SDR at 240 Hz, with a full-screen
120 FPS motion probe completing naturally after 190 seconds (22,800
presents per run), without a synthetic audio tone. These are two current
release repeats of the historical stock-client settings, not a new ABBA
comparison or a matched improvement claim against earlier releases.

Host windows begin five seconds after the first verified RUNNING API
sample and request 120 seconds. The unchanged analyzer uses the first
and last samples inside that interval, yielding the effective durations
below. Each run has 237 API samples at about 0.506-second intervals;
per-frame trace counts exactly match the frame-counter deltas.

| Run | Effective seconds | Frames / host sent FPS | Host video-byte counter Mbps | Send duration mean / p95 / p99 ms | Send gap p95 / p99 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 119.329 | 7,160 / 60.002 | 75.128 | 0.928 / 0.971 / 0.991 | 17.218 / 17.769 |
| 2 | 119.304 | 7,158 / 59.998 | 75.225 | 0.879 / 1.655 / 1.670 | 18.021 / 18.600 |

Both host windows had zero recorded replacements, IDR requests,
reference invalidations, loss recoveries, send stutters, trace drops,
writable waits, and send gaps longer than 1.5 frame periods. There were
no FEC reports, so exact packet loss is unavailable. The client-reported
global statistics cover its longer 165.017 / 165.045-second sessions:

| Stock client statistic | Run 1 | Run 2 |
| --- | ---: | ---: |
| Incoming frame rate | 60.05 FPS | 59.99 FPS |
| Decoding frame rate | 60.05 FPS | 59.99 FPS |
| Rendering frame rate | 59.99 FPS | 59.99 FPS |
| Host processing min / max / average | 2.1 / 3.6 / 2.5 ms | 2.1 / 3.8 / 2.5 ms |
| Network-dropped frames | 0.00% | 0.00% |
| Jitter-dropped frames | 0.03% | 0.01% |
| Average network latency (reported variance) | 1 ms (0 ms) | 1 ms (5 ms) |
| Average decoding time | 0.59 ms | 0.50 ms |
| Average frame queue delay | 0.02 ms | 0.02 ms |
| Average rendering time, including monitor V-sync latency | 0.48 ms | 0.47 ms |

These client statistics and host send measurements have distinct
windows and endpoints. They must not be added together or presented as
render-to-decoded picture age or glass-to-glass latency. The rounded
0.00% network-drop statistic is not proof of zero packet loss.

Both hosts stopped cleanly after the source completed and the stream
session cleared. **Both original client wrappers failed their immediate
cleanup check**, recording `Client cleanup or preservation check failed.`
and `processExited=false`. Separate checks at 09:23:48 and 09:30:26 UTC
confirmed that the owned client processes had exited without a second
close or force kill. Original preferences, all four original pairings,
the original GUI and Codex remained intact; the new benchmark pairing
was retained. The original wrapper failures remain in the evidence and
are not relabelled as passes. Final cleanup at 09:30:27 UTC found no
owned test processes, stream UDP endpoints or clock UDP endpoints.

`work\stock-summary.json` indexes the complete statistics, original
wrapper failures, later cleanup observations and verified evidence
hashes. Host evidence is under
`work\measurements\stock-laptop\stock-20261012T090903-553635Z`, in
`run-20261012T091735-588476Z` and `run-20261012T092522-181430Z`;
each retains its window, source ledger, samples, analyzer result and
all host log rotations archived after shutdown. Client evidence is in
`work\laptop-exchange\incoming\stock-r1-20261012T092445.zip`
(SHA-256 `87d6e69ec17a6e5693e54fea7614736b3cb393678bf253423eb1cd7d3ef729d3`)
and `stock-r2-20261012T093027.zip`
(`f47cbaeb8b8ff5a7ba79ccb5101daff6ad1686c7fc456b6d16a189107ebdca0d`),
with adjacent metadata/acknowledgements and
`stock-final-cleanup-20261012T093027.json`. Each archive retains all ten
stock statistics and a sanitized log; unsanitized client logs stayed
on the laptop. The summary verifies the upload, sanitized-log, source
ledger and archived-host-log hashes and preserves the fixture hashes.

### AV1 host-processing p95 diagnosis (October 12)

The morning DDX AV1 HDR result was **5.249 ms host mean**, with **8.4 ms
p95 in both repeats**, versus **3.109 ms mean / 3.2 and 3.4 ms p95** in
the retained October 8 rc.24 runs. At 120 FPS, 8.4 ms spans approximately
one 8.33 ms frame interval. The following investigation separates the
reported timer, release version, load phase and GPU dependencies.

**Timer boundary.** Native AMF starts the timer before capacity checking
and GPU conversion submission, then stops it when `QueryOutput` returns
the matching PTS. Unfinished capture-copy and conversion fences are inside
that interval. The sender reconstructs a claim timestamp from this duration;
it does not retain an independently sampled claim-to-packetization timer.
Output-buffer copying occurs after the AMF sample and before that
reconstruction. The inline sender then adds the small interval to its
pre-packetization sample; the separate paced sender also includes residence
in its sending queue and waiting for sender capacity. Only the frame-header
value rounds to 0.1 ms. These loopback runs used inline sending. In these runs,
the reported host duration matched the AMF stage duration within the
wire rounding. Therefore the observed increase cannot be called slower
hardware compression from the host counter alone.

**Released-binary control.** Run the original rc.24 and 2.2.9 packages in
A/B/B/A order on the same current desktop: RX 7900 XT, physical
5120×1440 SDR at 240 Hz, native 1968×2184 HDR source at 120 Hz, AV1 HDR
120 FPS requesting 80 Mbps, identical AMF settings and four requested
decoder threads. The established load is `gpu_load 45 1000 60 200`.
Each receiver runs for 35 seconds with five seconds excluded as warmup.

| Execution order | Host mean / p95 (ms) | Render-to-decoded picture age mean (ms) |
| --- | ---: | ---: |
| rc.24 A1 | 3.444 / 7.1 | 12.362 |
| 2.2.9 B1 | 3.198 / 4.1 | 10.477 |
| 2.2.9 B2 | 3.019 / 3.3 | 9.123 |
| rc.24 A2 | 3.043 / 3.3 | 11.198 |

The old release also develops the slow tail. These controls do not reproduce
a persistent 2.2.9 increase from 3.11 to 5.25 ms. The historical desktop and
run phase changed, so the earlier difference is not an isolated code-regression
measurement. Reducing the current receiver's requested decoder threads to one
gave **3.117 / 3.4 ms host mean / p95**; both thread settings reported
`active_thread_type=0` and `delay=0`.

**Load phase.** Temporary tracing builds preserve each release's algorithms
and add per-frame stage observations. A current DDX trace reproduced
**4.909 / 8.0 ms host mean / p95**: all 1,318 receiver samples at or above
6 ms belonged to even source IDs. The morning slow phase primarily affected
odd IDs. Changing only the load cadence from 60 to 59 FPS moved the
host-duration periodic component to 59 Hz and made slow-frame parity switch
on a one-second beat. The two 59 FPS runs measured **3.347 / 6.5 ms** and
**3.405 / 6.3 ms**. This ties the slow tail to the relative timing of the
GPU workload and capture, rather than fixed odd/even image complexity.

**Where the time goes.** Receiver-aligned stages in the slow 60 FPS trace
averaged **7.507 ms**, versus **3.388 ms** for frames below 6 ms. Every one
of its 1,317 stage samples at or above 6 ms had an unfinished capture-copy
fence at submission. CPU conversion submission averaged **0.047 ms**,
AMF submission **0.120 ms**, and the p95 of each slow frame's longest
post-submission polling gap was **0.587 ms**. The old traced release's
slow frames showed the same pending-copy pattern. The one-sample difference
between the stage and receiver counts above comes from applying the 6 ms
threshold before versus after 0.1 ms wire rounding.

A second tracing build also sampled the producer/acquired fence and the
destination texture's prior-reader fence. Across three DDX runs, **all 149
slow steady frames** had an unfinished producer fence at submission and an
already completed prior-reader fence. There were **zero slow frames with
a pending prior-reader fence**, and each previous capture copy was complete
at submission. Per-run means of the first observations of
producer completion were **4.831 / 5.068 / 5.238 ms** after encoder entry;
copy completion was first observed at **4.995 / 5.263 / 5.542 ms**, and
encoded output at **7.182 / 7.381 / 7.828 ms**. These are CPU-observed
completion bounds, not exact GPU execution timestamps. They locate the
dominant added wait at the D3D11 producer/acquisition dependency, before
the compute copy, conversion and AMF compression. These CPU observations do
not separate DWM execution from scheduling delay; the deeper ETW analysis
[below](#dwm-dependency-and-early-signal-experiment) follows that dependency.

The independent GPU ETW trace corroborates this ordering. For slow PTS 2583
(capture-copy fence 2590), the host's Compute0 queue waited **3.541 ms**
for a producer signal from its graphics queue. The copy packet was released
after that wait and completed in **0.284 ms**. The multi-millisecond stall
therefore precedes execution of the compute copy itself. All **13 slow
frames** in the selected 120-frame trace window joined to this producer
dependency: wait mean **3.073 ms** (range **2.465–3.541 ms**), followed by
copy DMA release-to-completion mean **0.152 ms**. The other 107 frames'
copy interval averaged **0.160 ms**. These packet intervals are not the
whole AMF frame duration.

The producer wait preserves correctness: Desktop Duplication can return
before DWM has finished the image. The retained [October 4 synchronization
probe](PERFORMANCE.md#october-4-capture-copies-and-conversion-beside-a-game)
found **316/325 mismatches under load without that wait**, versus **0/267
with it**. Removing the fence or starting the reported timer after it would
not be a demonstrated latency fix. Faster output polling and different AQ
settings do not target the multi-millisecond dependency identified here.

**Existing capture alternative.** Repeat DDX/WGC/WGC/DDX with the 59 FPS
load so that relative phase walks through the full cycle. Both use the
same dependency-tracing binary and stream settings. Values below average
the two runs' statistics; they are not pooled percentiles.

| Capture | Host mean / p95 (ms) | Picture age mean / p95 (ms) | Fresh pictures/s |
| --- | ---: | ---: | ---: |
| DDX | 3.126 / 3.7 | 10.597 / 15.116 | 118.924 |
| WGC | 3.038 / 3.4 | 11.345 / 15.580 | 119.936 |

WGC had no host samples at or above 6 ms in either repeat, but its picture
age mean was **0.747 ms higher**. The first DDX run delivered 117.840 fresh
FPS with 64 source discontinuities; the second delivered 120.009 with none.
This establishes a capture/cadence tradeoff, not a general end-to-end latency
improvement. A future DDX optimization must reduce the producer dependency
while retaining synchronized pixels, picture freshness and throughput. Moving
the producer signal earlier to overlap command-list preparation was tested
in the [four-run experiment below](#dwm-dependency-and-early-signal-experiment).
It did not establish a real picture-age improvement. No
production algorithm or default was changed by this diagnosis.

**Evidence and validation.** All **17** diagnostic cases completed their
ordinary stream checks and restored the physical display. All **70,535**
nonzero source-ID/QPC pairs matched their original render ledgers exactly;
**136 unreadable rows** remain in the original denominators. The one GPU/CPU
WPR case had zero lost trace events/buffers, but delivered 117.49 fresh FPS
with additional source discontinuities; its performance is kept separate.
Diagnostic builds and their logging are not replacement released-package
headline measurements. The original morning and competitor tables remain
unchanged.

Local evidence is under `work\host-mean-trace`: the twelve-case
`run-20261012T150121Z`, five-case `dependencies-20261012T152710Z`,
`receiver-analysis.json`, `phase-analysis.json`, `stage-cause-summary.json`,
`receiver-analysis-dependencies-20261012T152710Z.json` and the second batch's
`analysis\stage-cause-summary.json`. Each batch retains source ledgers,
receiver CSVs, stage logs, source/binary hashes and display snapshots. The
matched GPU queue records are in `etw\gpu-window-analysis.json`. The
release sources are rc.24 `ec84f48abbea67e0e3c3f8fc0c7c71e090831580` and
2.2.9 `d0bfcb5e764095250b8f0e22fd5a0cb3656d5ea6`. Temporary source changes,
builds, raw evidence and credentials remain outside the repository.

#### DWM dependency and early-signal experiment

Following the producer fence upstream in the same retained ETW window
identified **unfinished `dwm.exe` graphics work in all 13 slow frames**.
After DWM's fence completed, the host wait completed within **6–74 µs**;
its producer signal then completed **3–24 µs** later. The multi-millisecond
delay is therefore upstream of host signal completion.

For PTS 2583, DWM submitted its preceding graphics DMA packet at trace time
37.115663 s. That packet was released at 37.118722 s and completed at
37.119220 s: **3.059 ms before release**, followed by **0.498 ms** to
completion. DWM's signal was queued at 37.115673 s but only completed at
37.119237 s. The host wait completed at 37.119271 s, and its producer signal
at 37.119295 s. Queuing a signal must not be mistaken for completing it.

Twelve of the thirteen slow frames' preceding DWM packets spent
**2.423–3.412 ms between submission and release**. The remaining packet
spent **3.358 ms between release and completion**. All thirteen overlapped
two graphics packets from the synthetic GPU load. No explicit wait packet
on DWM's queue appeared in the retained window. An adjacent fast DWM packet
took **31 µs** from submission to release and **104 µs** from release to
completion. These events identify DWM graphics scheduling/completion as
the upstream dependency; they do not identify a particular DWM shader or
measure pure GPU execution time. The command-buffer byte count is not an
image-copy size.

**Candidate and acceptance rule.** A scratch build moved the D3D11 producer
`Signal` and `Flush` before shared-resource opening and copy-command-list
preparation. Every synchronization wait, resource lifetime and timing
boundary remained intact. Before running it, the plan required repeated
reductions in host p95 and producer wait, without deterioration in picture
age or fresh-picture cadence. Both arms used identical stage tracing,
native HDR settings and the 59 FPS load described above. Four serial runs
used unchanged/candidate/candidate/unchanged order:

| Run | Host mean / p95 / p99 (ms) | Picture age mean / p95 (ms) | Fresh pictures/s | Host samples ≥6 ms |
| --- | ---: | ---: | ---: | ---: |
| Unchanged A1 | 3.103 / 3.4 / 7.0 | 11.240 / 15.225 | 119.969 | 54/3582 |
| Earlier signal B1 | 3.061 / 3.3 / 4.6 | 10.859 / 15.003 | 120.003 | 27/3571 |
| Earlier signal B2 | 3.089 / 3.3 / 6.9 | 11.208 / 15.245 | 119.778 | 50/3576 |
| Unchanged A2 | 3.138 / 3.4 / 7.5 | 10.896 / 14.962 | 119.979 | 81/3581 |

Host p95 decreased by **0.1 ms** in both candidate runs. However, the
picture-age improvement did not repeat: B1 versus A1 improved its mean by
0.382 ms, while B2 versus A2 increased it by 0.312 ms. Averaging each arm's
two run statistics, picture-age mean changed by **−0.035 ms** and p95 by
**+0.031 ms**; these are not pooled percentiles. B2 omitted six source
pictures, versus one in either control. This does not establish
candidate-caused degradation, but it fails the combined acceptance rule.

All **208 stage samples at or above 6 ms** still had a pending producer
fence and a completed prior-reader fence. Per-run first-observed producer
completion means were **4.944 / 5.062 / 4.776 / 5.007 ms** after encoder
entry. These CPU-observed bounds show no consistent shortening of the
slow dependency. The counts at or above 6 ms differ between stage and wire
measurements because of 0.1 ms wire rounding. The candidate was **not adopted**, and the conditional
60 FPS confirmation was not run. Acquisition timing remains an untested
optimization direction; neither bypassing the correctness fence nor moving
the timer past it would resolve this measured delay.

**Validation and provenance.** All four stream gates, binary-hash checks,
native-source modes and physical-display restoration checks passed.
All **16,646 nonzero source-ID/QPC pairs** matched exactly; **49 zero-ID
startup rows** remain in the raw evidence. All **14,310 steady frames**
mapped to stage records. No production source, installed binary, capture
default or migration behavior changed.

The batch is `work\host-mean-trace\early-signal-20261012T163740Z`, with
`analysis\early-signal-comparison.json` and adjacent raw logs and ledgers.
The parent directory retains `early-signal-plan.json`,
`early-signal-receiver-judgment.json` and
`receiver-analysis-early-signal-20261012T163740Z.json`. Exact DWM fence/value
joins are in `etw\producer-chain-analysis.json` and
`etw\producer-chain-summary.json`. The instrumented unchanged host SHA-256
is `9e11727043573faecdaf536f3bedb785407852a5e888267f4eefd04f8b51fc1a`;
the candidate is
`3a001534068d0756ee8b742eccc53aa0a8c0c2c939fb2edaeae1aa8c04277182`.

#### DDX post-publication hold experiment

A second scratch candidate tested frame ownership timing. After publishing
each DDX snapshot and waking encoders, it requested one existing polling
interval (**500 µs**) before the next release/acquire. The timer can overshoot;
500 µs is not a measured maximum hold. The current picture still publishes
immediately, and all GPU waits, texture-ownership rules and encoder timer boundaries remain
unchanged. The earlier-signal change was removed. This tests whether holding
the duplication surface briefly changes subsequent DWM work; it does not
assume that the retained trace contains redundant desktop copies.

The same instrumented baseline, native HDR settings and 59 FPS load were
run in A/B/B/A order. Acceptance required repeatable picture-age and host-tail
improvement with fresh-picture cadence preserved, before any 60 FPS
confirmation or production change.

| Run | Host mean / p95 / p99 (ms) | Picture age mean / p95 (ms) | Fresh pictures/s | Omitted source pictures |
| --- | ---: | ---: | ---: | ---: |
| Unchanged A1 | 3.109 / 3.5 / 6.6 | 10.808 / 14.853 | 118.880 | 33 |
| Hold B1 | 3.058 / 3.3 / 4.4 | 10.136 / 14.636 | 120.001 | 0 |
| Hold B2 | 3.166 / 4.1 / 7.5 | 10.411 / 14.616 | 119.979 | 1 |
| Unchanged A2 | 3.498 / 7.2 / 8.0 | 10.342 / 13.740 | 119.998 | 0 |

The first candidate run improved on the first control, but the second
increased picture-age mean by **0.069 ms** and p95 by **0.876 ms** against
the final control. Both candidate picture-age p95 values exceeded the final
control, despite its higher host p95. Across all four runs, **544/544 stage
samples at or above 6 ms** still had a pending producer fence, a completed
prior-reader fence and a completed previous copy. Slow-frame first-observed
producer completion means were **4.648 / 4.843 / 4.688 / 4.709 ms** after
encoder entry: the same multi-millisecond dependency remained. These are
CPU-observed bounds, not GPU execution durations.

The acceptance rule was not met, so the hold was **not adopted** and the
conditional 60 FPS confirmation was not run. Baseline variation also
prevents claiming that the candidate caused a regression. No production
code, capture defaults or installed settings changed.

All four ordinary stream checks, binary hashes, native source modes and
physical-display restoration checks passed. All **16,643 nonzero source
ID/QPC pairs** matched their render ledgers, and **14,293 steady frames**
aligned to stage records without flags. The **47 zero-ID startup rows**
remain in the evidence. Raw source omissions were retained in the results.

Evidence is `work\host-mean-trace\posthold-20261012T174145Z`, including
`analysis\posthold-comparison.json`, the predeclared plan, source hashes,
logs and ledgers. The parent directory retains
`receiver-analysis-posthold-20261012T174145Z.json`. The baseline host hash
is unchanged from the earlier-signal experiment; the hold candidate SHA-256
is `97a570378033c0cbbedf33c7785903bfb2072b69c9ed847b036074e961823612`.

### 2026-10-17: negotiated HEVC intra-refresh candidate

For [issue #36](https://github.com/RamazanKara/Rubylight/issues/36), adopt the
finite-GOP/whole-CTB-row workaround only when the client negotiates HEVC
intra-refresh. At 3840x2160 this requests GOP 120 and 60 CTBs per slot. Both
settings are verified before and after AMF initialization. Rejected settings
restore the previous pair; an initialization failure with the candidate gets
one fresh encoder attempt using the previous policy. Startup and recovery
IDRs remain enabled, and the implementation never sets `HevcGOPSPerIDR=0`.

Native validation used an RX 7900 XT, AMD driver 32.0.32015.2008, D3D12 encoder
input, 3840x2160 at 119.88 FPS, 117,964 kbps and a one-reference client budget.
Each SDR/HDR case submitted 900 frames: 240 static frames followed by 660
frames alternating four detailed synthetic GPU textures. A forced recovery
IDR at frame 733 exercised recovery away from the GOP boundary. Independent
FFmpeg decoding checked the complete stream and a separate stream beginning
at that recovery IDR.

| Check | SDR | HDR |
| --- | ---: | ---: |
| Complete stream decoded | 900/900 | 900/900 |
| Recovery tail decoded | 167/167 | 167/167 |
| Largest encoded frame | 210,924 bytes | 185,617 bytes |
| Largest-frame FEC layout (data + parity) | 154 + 31 | 135 + 27 |
| FEC blocks / requested protection | 1 / 20% | 1 / 20% |

Both cases read back GOP 120, GOPs per IDR 1, CTBs per slot 60 and one reference.
Only the requested startup/recovery frames were IDRs; no periodic IDR/CRA
bursts appeared. Every submission produced one frame in order. No decoder
errors, stalls, packet-limit violations or reduced FEC protection occurred.
Packet budgets used the production packetizer with packet size 1392 and
minimum parity 2. Unit checks passed: 502 core tests, 22 AMF tests, and Clippy
for core/Windows libraries and tests with warnings denied.

These checks establish local encoding and recovery compatibility. They do
not establish a latency or picture-quality gain, network behavior, or a fix
on the reporter's Xbox Series X / RX 9070 XT. No capture, display or network
was involved, and no installed host settings changed. The separate HDR-toggle
report remains open; the later Series S check below provides client visual
validation.

The reproducible ignored native test is
`amf::intra_refresh_native_tests::native_hevc_intra_refresh_decodes_across_gops_and_forced_recovery`.
Set `BUTTERPOLLO_TEST_FFMPEG` to an independent FFmpeg executable and
`BUTTERPOLLO_TEST_HEVC_INTRA_REPORT` to an absolute task-owned JSON path, then
run that test with `--ignored --nocapture --test-threads=1` on an idle AMD GPU.
Local evidence is retained in
`D:\CodexArtifacts\rubylight-hevc-intra-20261017`, including `candidate.json`,
native logs, full/recovery bitstreams and independent decode results.

### 2026-10-17: Xbox Series S HEVC visual confirmation

The user previously reproduced HEVC colour smearing on Xbox Series S with
the latest Xbox Moonlight client. After installing the 2.5.3 candidate, the
user replayed the scene and reported **"Smearing is gone."** Its HEVC code
is unchanged in the combined 2.5.4 release.

The live host log identifies an RX 7900 XT streaming 3840x2160 HEVC HDR at
119.88 FPS through WGC and AMF with D3D12 input. AMF read back GOP 120,
GOPs per IDR 1, CTBs per slot 60 and one reference. Host timing records
showed one startup IDR request and no reported recovery activity during
the check. The client supplied no FEC status reports, so these host counters
cannot establish zero client packet loss.

This is a user visual comparison plus host diagnostics. It is not an
independent client decode count or a controlled latency/quality benchmark,
and does not establish the result on Xbox Series X / RX 9070 XT or resolve
the separate HDR-toggle report. The session began at 20:03:44 UTC.
Filtered host logs and window-level diagnostics are retained under
`D:\CodexArtifacts\rubylight-release-2.5.4-20261017` as `xbox-session.log`
and `xbox-check.json`.

## 2026-10-17 AMF D3D12 input state reuse

The Xbox session later stalled at 20:11 UTC and ended in
`DXGI_ERROR_DEVICE_RESET`. Windows reported the RX 7900 XT in Code 31 until
a restart. The reconnect message "Another stream operation is still running"
was emitted while native encoder/display cleanup remained blocked. Cleanup
eventually completed, but the unavailable adapter prevented a new encoder
from starting. The saved live kernel reports identify an AMD video-engine
timeout (`0x141`, `amdkmdag.sys`) followed by a failed adapter add (`0x1B0`).
These reports do not diagnose faulty hardware or establish the initiating cause.

Four reset episodes in the installed host log, on October 10, 12, 14 and 17,
used WGC, HDR and D3D12 AMF input on this adapter. Both HEVC and AV1 were
affected; the earlier episodes predate the negotiated HEVC intra-refresh
workaround. This is a pattern on one test host, not a population failure rate.

An independent short native check found a resource-state defect. AMF records
the current state on the input resource through `AMFResourceStateGUID` and
leaves these encoded textures in `COPY_SOURCE` (`0x800`). The converter
previously reused them with a `COMMON -> UNORDERED_ACCESS` barrier. The
Direct3D 12 debug layer reported error 527,
`RESOURCE_BARRIER_BEFORE_AFTER_MISMATCH`, for each plane of the reused texture.
The [AMF synchronization contract](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/public/include/core/D3D12AMF.h)
and [AMD sample transition helper](https://github.com/GPUOpen-LibrariesAndSDKs/AMF/blob/master/amf/public/samples/CPPSamples/common/SwapChainDX12.cpp)
require using the recorded current state.

New conversion targets now initialize that metadata to their actual `COMMON`
state. Reuse reads AMF's recorded state after the existing texture ownership
and completion-fence checks. The conversion transitions from that state,
and returns to `COMMON` for the next synchronized submission. Missing or
malformed state metadata returns an error instead of guessing a barrier.
GPU waits, frame limits, encoder settings and colour conversion remain the
same. The debug layer is enabled only in the opt-in native regression test.

Validation on RX 7900 XT, AMD driver `32.0.32015.2008`:

| Check | Before | After |
| --- | ---: | ---: |
| HDR HEVC + AV1, 32 frames each, Direct3D errors | 124 | 0 |
| SDR/HDR HEVC + AV1, 32 frames per case, Direct3D errors | not rerun | 0 |
| Independent decoding, four cases | not rerun | 128 / 128, no decoder errors |

The native fixture uses synthetic 3840x2160 textures, codec configuration
119.88 FPS and one reference. It submits one frame at a time without stream
pacing. These are state-validation and decode checks, not FPS or latency
benchmarks. HEVC uses FFmpeg's `hevc` demuxer; the low-overhead AV1 stream
uses `obu`, with strict error detection and frame hashes. The existing
compute-versus-graphics colour conversion test also passed. All 318 ordinary
Windows unit tests passed; 63 native tests remain opt-in.

The defect is reproduced and corrected. These short checks cannot prove it
caused every recorded reset or establish long-session stability. The visual
HEVC smearing result above remains valid, with this later stability incident
recorded separately. The correction is included in 2.5.5 and is not in the
published 2.5.4 binaries. Saved dumps, debugger analyses, before/after validation JSON,
encoded streams and decoder outputs are under
`D:\CodexArtifacts\rubylight-release-2.5.4-20261017`.
