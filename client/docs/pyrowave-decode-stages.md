# PyroWave decode on the desktop client

[Documentation](index.md)

Measurements of Nightjar's PyroWave decode and present path on Windows. Everything here is our own measurement on one PC (RX 7900 XT, loopback to
an isolated Rubylight host), not a claim about other hardware or other clients.

## How it is measured

`scripts/pyrowave-loopback.ps1` runs the whole protocol:

- An isolated Rubylight host (its own config dir, port 48123, `virtual_display_layout = extended`)
  streams a scrolling texture (`scripts/pyrowave-anim.ps1`) at 1080p60, PyroWave 4:2:0.
- The client starts itself with `NIGHTJAR_AUTOSTREAM=127.0.0.1:48123`, pairs once, streams for
  `NIGHTJAR_AUTOSTREAM_SECONDS` and exits. `NIGHTJAR_CONFIG_DIR` keeps its settings and pairing
  away from the real profile. Both are for measurement; a normal start ignores them.
- One timing CSV (`NIGHTJAR_TIMING_CSV`) and one log per leg. Legs run A B B A per bitrate, 60 s
  each, the first 5 s discarded, early decode off (`NIGHTJAR_EARLY_DECODE=0`).
- The script takes `C:\src\butterpollo-release\GPU-LOCK` (created atomically, polled twice a second)
  and looks at the installed host again once it holds the lock. Exit codes: 2 for a failed or hung
  leg (after a scan of the host log for a hang), 3 for a suspected AMD encoder hang, 4 when the
  installed host does not answer and its service is not stopped.
- The client window is kept out of the host's screen capture (`NIGHTJAR_EXCLUDE_FROM_CAPTURE=1`,
  `SetWindowDisplayAffinity` with `WDA_EXCLUDEFROMCAPTURE`) because this PC has one display. Without
  that the client draws into the picture it is streaming and the rate follows what it shows (see
  the fps question below). `-CaptureVisible` turns it off; `-Codec Hevc` streams HEVC instead of
  PyroWave.
- `scripts/timing_summary.py` summarizes the legs. Percentile rule:
  `sorted[min(n-1, round_half_up(p/100*(n-1)))]`. Shown fps is `(n-1)/(last-first)` of the
  `render_ns` stamps per leg, stalls included, then the mean of the legs. The CSV's `shown_fps`
  column is now filled too (from the last second of presents).

Keep rule for a fix (B against A, per bitrate): both B legs beat both A legs on the target metric's
p50, the pooled p95 is not worse, `receive_to_present_ms` p95 is at most 0.5 ms worse and shown fps
at most 0.5 lower. Otherwise the fix is reverted. The target metric here is
`decode_to_present_ms`.

Stage timers (printed at session end as `PyroWave decode stages` and `PyroWave presenter stages`):

| Stage | Where | What |
| --- | --- | --- |
| assemble | decoder thread | slices into the frame's envelope |
| clear | decoder thread | zeroing the coefficient planes |
| entropy | decoder thread | entropy decode into coefficients |
| pack | decoder thread | RGBA16F frame fill and half packing, after alloc |
| alloc | decoder thread | the RGBA16F frame allocation |
| upload | presenter | `SDL_UpdateTexture` of the coefficient frame |
| passes | presenter | CPU time to submit the 11 wavelet passes (not GPU time) |

With the GPU decoder (`pyrowave_decoder = Gpu`, docs/pyrowave-decode.md) the decode line reads
`clear`, `push` (records into the SDK), `submit` and `gpu` (the SDK's own pass timing) instead, and
the presenter's `upload` is 0: the planes are already on the GPU. `scripts/pyrowave-loopback.ps1`
takes `-DecoderA/-DecoderB Cpu|Gpu`, `-Width/-Height/-Fps` and `-Port` for those runs.

`NIGHTJAR_PYROWAVE_DUMP=<dir>` writes the first 120 real envelopes; the ignored test
`cpu_costs_of_a_pyrowave_frame` reads them back through `NIGHTJAR_PYROWAVE_FRAMES`.

## Baseline stage split (before any fix)

One A leg per bitrate, ms p50/p95 over about 3500 frames.

| Stage | 133 Mbps setting | 300 Mbps setting |
| --- | --- | --- |
| assemble | 0.02 / 0.04 | 0.02 / 0.07 |
| clear | 0.92 / 1.27 | 0.98 / 1.64 |
| entropy | 2.45 / 3.42 | 3.02 / 6.93 |
| pack | 5.03 / 8.66 | 5.97 / 13.77 |
| alloc | 0.01 / 0.03 | 0.02 / 0.04 |
| upload | 3.53 / 4.73 | 3.95 / 5.44 |
| passes (submit) | 0.13 / 0.21 | 0.15 / 0.26 |

Client metrics of the same two legs:

| Metric | 133 | 300 |
| --- | --- | --- |
| input_to_output_ms p50/p95/p99 | 8.62 / 13.83 / 20.33 | 10.24 / 21.97 / 32.69 |
| receive_to_input_ms p95/p99 | 1.80 / 9.52 | 24.95 / 50.95 |
| queue_wait_ms p95/p99 | 1.25 / 8.91 | 24.35 / 50.36 |
| decode_to_present_ms p50/p95 | 54.28 / 57.53 | 54.90 / 60.63 |
| receive_to_present_ms p50/p95/p99 | 63.59 / 73.16 / 82.61 | 66.52 / 100.97 / 123.09 |

The decoder dropped no frame over its queue in either leg; 12 and 15 frames were replaced before
they were shown.

Two cautions on reading this:

- The "133" and "300" are the bitrate set in the client. The host's own `sent_mbps` stayed near
  85 to 90 Mbps in every leg of both settings (median 83 to 90), so with this content the two
  settings did not produce different traffic. The difference between the columns is mostly run
  noise; the 300 baseline leg has a heavy `queue_wait_ms` tail that the later 300 legs (A legs of
  the fix runs, p95 63 to 72 ms `receive_to_present_ms`) do not show.
- `decode_to_present_ms` sits near 52 to 54 ms in every leg and barely moves with the stage times
  (a fix that saves 1.7 ms of upload moves it by 1.5 ms, one that saves 0.06 ms of submit by 0).
  We have not split what that interval holds besides the measured stages.

What the stages say: pack (5 ms p50) and upload (3.5 ms) are the largest measured stage costs, then
entropy (2.5 to 3 ms), then clear (0.9 ms). Pass submission is 0.1 ms of CPU time.

## Fix (a): upload to one texture

The reconstruction frame was uploaded to both `textures[0]` and `textures[1]`. Pass 1 reads
`textures[1]` only inside the area pass 0 just wrote, so only `textures[0]` needs the upload. The
ignored test `single_upload_draws_the_same_pixels_as_uploading_both_textures` draws a frame both
ways and compares every pixel.

| Group | Leg | decode_to_present p50 / p95 | receive_to_present p50 / p95 / p99 | shown fps |
| --- | --- | --- | --- | --- |
| 133 | A 1 | 53.59 / 55.32 | 60.14 / 65.20 / 71.53 | 57.34 |
| 133 | B 1 | 52.28 / 53.38 | 59.31 / 64.20 / 69.69 | 58.52 |
| 133 | B 2 | 52.45 / 53.67 | 59.88 / 68.89 / 87.24 | 59.28 |
| 133 | A 2 | 54.24 / 56.09 | 62.57 / 73.94 / 110.12 | 58.80 |
| 133 | A pooled | 53.99 / 55.74 | 61.36 / 69.97 / 90.05 | 58.07 |
| 133 | B pooled | 52.38 / 53.53 | 59.61 / 65.99 / 81.43 | 58.90 |
| 300 | A 1 | 54.26 / 56.49 | 62.61 / 72.32 / 86.30 | 58.71 |
| 300 | B 1 | 52.38 / 54.35 | 60.12 / 78.49 / 117.19 | 57.86 |
| 300 | B 2 | 52.17 / 53.24 | 58.89 / 62.99 / 66.80 | 59.15 |
| 300 | A 2 | 48.43 / 54.65 | 55.15 / 63.18 / 65.21 | 56.24 |
| 300 | A pooled | 53.81 / 55.64 | 60.89 / 68.14 / 80.16 | 57.48 |
| 300 | B pooled | 52.27 / 53.80 | 59.41 / 69.80 / 99.95 | 58.51 |

Upload stage, ms p50/p95: A legs 3.17 to 3.59 / 4.16 to 4.74, B legs 1.65 to 1.85 / 2.37 to 2.79.

Second 300 Mbps run (ABBA again, same binaries):

| Leg | decode_to_present p50 / p95 | receive_to_present p50 / p95 / p99 | shown fps |
| --- | --- | --- | --- |
| A 1 | 54.04 / 55.69 | 62.29 / 68.29 / 73.80 | 59.20 |
| B 1 | 48.89 / 53.07 | 58.01 / 63.28 / 69.22 | 56.58 |
| B 2 | 52.58 / 54.30 | 60.95 / 77.52 / 95.59 | 59.38 |
| A 2 | 53.66 / 55.50 | 60.48 / 66.46 / 74.18 | 57.65 |
| A pooled | 53.89 / 55.60 | 61.51 / 67.66 / 73.97 | 58.43 |
| B pooled | 52.27 / 53.74 | 59.56 / 70.60 / 87.59 | 57.98 |

Verdict: **kept**, as a judgement call.

- 133 Mbps: passes the rule.
- 300 Mbps, first run: fails. One A leg (A 2) was an outlier at 48.43 ms p50, so B did not beat both
  A legs, and `receive_to_present_ms` p95 was 1.67 ms worse.
- 300 Mbps, repeat: B wins both legs on the target `decode_to_present_ms` p50 and on its p95, but
  `receive_to_present_ms` p95 is 2.94 ms worse (B 70.60, A 67.66). That tail comes from the
  decoder's `queue_wait_ms`, which the presenter change does not touch, and the shown fps is within
  0.5 of A only by a margin.

The change removes 1.7 ms from the upload stage in all six B legs and costs the presenter nothing,
so it stays, but it did not pass the 300 Mbps rule strictly. It is one commit; reverting it is
`git revert e385b59c`.

## Fix (b): the eleven passes as one command list

Batching the 11 `d3d11::Filter` draws into one command list. It was built, tested for identical
pixels, measured, and **reverted** (`8dbe2dfb`).

| Group | Variant | decode_to_present p50 / p95 | receive_to_present p50 / p95 / p99 | shown fps |
| --- | --- | --- | --- | --- |
| 133 | A pooled | 52.42 / 53.80 | 61.26 / 69.11 / 77.27 | 58.92 |
| 133 | B pooled | 52.40 / 53.66 | 61.21 / 68.37 / 74.46 | 59.39 |
| 300 | A pooled | 51.94 / 53.07 | 58.12 / 62.51 / 66.40 | 56.67 |
| 300 | B pooled | 49.35 / 52.97 | 56.16 / 62.16 / 65.85 | 56.46 |

The stage timer shows what the change did: the submit cost of the passes fell from 0.10 to 0.04 ms
p50. But `decode_to_present_ms` p50 did not move at 133 (52.40 against 52.42) and at 300 one B leg
(43.43) beat both A legs while the other (51.91) lost to one of them (46.99), so the keep rule says
revert at both bitrates. The CPU time it saves is under 0.1 ms per frame; that is not where the
time is.

## Why decode_to_present_ms sat at 53 ms: the present clock's start

Every A leg above (and the relaunch debugging runs) showed `decode_to_present_ms` p50 near 53 ms,
p95 near 54.5 ms, while `input_to_output_ms` was 6 ms and `queue_wait_ms` tiny. It is real latency,
not a measurement artefact, and it comes from the present clock, not from the swap chain.

- The stream runs with pacing `HostTimes` (the log line `streaming ... pacing HostTimes`). That is
  the mode when `vrr` and `present_at_host_times` are both on. Since the measurement in
  docs/pyrowave-decode.md ("The wait between decoded and shown") `present_at_host_times` is off by
  default and VRR displays present immediately; the numbers below are from when it was the default. SDL vsync is off in this mode, so no
  vsync queue or swap chain frame latency is involved.
- `PresentClock` (`protocol/core/src/present_clock.rs`) shows each frame at its host timestamp plus
  a delay. The delay is the 97th percentile of how late recent frames were, plus 0.5 ms. It rises at
  once and falls by 25 us per frame (1.5 ms per second at 60 fps). A frame is held at most
  `max_hold_us`, 50 ms.
- The first pictures of a stream wait for the decoder and the GPU to warm up and arrive as a burst.
  In the 133 Mbps relaunch run, frame 1 was 91.9 ms later than the steady state (its ready time
  against its host timestamp), and frames 2 to 7 never reached the screen (replaced by frame 8).
  The first schedule seeded the delay from frame 1. From then on every picture was held for the
  longest hold, 50 ms, until the delay had fallen by tens of milliseconds at 1.5 ms per second.
- The timeline of one leg shows it (5 s buckets, `decode_to_present_ms` median): 53.9, 53.6, 53.6,
  53.7, 53.6 (25 s), 53.5, then 48.3, 42.0 and 35.5 ms at 40 s, still falling when the leg ended at
  45 s. In a 300 Mbps leg the median stayed at 54.0 to 54.9 ms for the whole 60 s.
  A queue in front of the present (swap chain buffers, frame latency) would not drift down like
  this, and it would not depend on how late the first picture was.
- A leg of brief 2's run with fewer shown frames also had a shorter delay: what decides the delay
  is how the first frames came, not a fixed offset.

First fix (`2c52af1b`, replaced by the rule in the next section): the first 16 pictures after the start, or after a restarted video clock, are
shown as soon as they are ready (plus the 0.5 ms margin) and are not entered in the lateness
window. The regression test `a_late_burst_at_the_start_does_not_hold_the_stream` held frame 30 for
50415 us before the change. All other `present_clock` tests pass unchanged. Android's engine uses
the same clock.

ABBA, 60 s legs, first 5 s dropped, early decode off, A is the build before the change
(`B-upload.exe`), B has it (`C-clock.exe`). Times in ms.

| Group | Leg | decode_to_present p50 / p95 | receive_to_present p50 / p95 / p99 | shown fps |
| --- | --- | --- | --- | --- |
| 133 | A 1 | 45.59 / 52.48 | 51.95 / 59.23 / 61.49 | 55.39 |
| 133 | B 1 | 6.39 / 19.85 | 12.59 / 26.46 / 28.74 | 50.67 |
| 133 | B 2 | 15.17 / 20.40 | 22.64 / 28.18 / 29.51 | 50.51 |
| 133 | A 2 | 48.33 / 52.74 | 54.84 / 60.17 / 61.56 | 56.17 |
| 133 | A pooled | 46.96 / 52.62 | 53.81 / 59.89 / 61.55 | 55.78 |
| 133 | B pooled | 11.46 / 20.27 | 18.58 / 27.40 / 29.14 | 50.59 |
| 300 | A 1 | 52.55 / 54.04 | 60.27 / 92.46 / 125.62 | 58.61 |
| 300 | B 1 | 20.39 / 53.72 | 29.39 / 68.17 / 91.04 | 54.32 |
| 300 | B 2 | 15.00 / 19.96 | 23.81 / 29.12 / 32.13 | 51.78 |
| 300 | A 2 | 52.07 / 53.20 | 59.16 / 63.49 / 67.91 | 56.99 |
| 300 | A pooled | 52.31 / 53.75 | 59.71 / 75.04 / 115.26 | 57.80 |
| 300 | B pooled | 16.51 / 53.33 | 25.49 / 65.11 / 82.07 | 53.05 |

The latency result is large and in every B leg: `decode_to_present_ms` p50 falls from 47 to 52 ms
to 6 to 20 ms, `receive_to_present_ms` p50 from 54 to 60 ms to 13 to 29 ms. In one 300 Mbps B leg
(06) the delay rose back to the 50 ms hold after 40 s (the median goes 26, 19, 47, 53, 53, 53 ms
from 30 s on), after a stretch of late frames; the clock does what it was built to do there.

The keep rule said **revert** on the shown fps criterion for this first fix (50.6 against 55.8 at
133, 53.1 against 57.8 at 300). That table was measured with the client window inside the host's
capture, and the next section shows the fps gap was an artefact of that. What we knew then:

- The frames the client shows are the frames the host sent, not frames the client dropped. B legs
  received 3009 and 3031 frames at 133 Mbps against 3308 in the first A leg, with 6 to 12 replaced
  before they were shown in every leg; the host's `sent_mbps` was 74 to 81 in B legs against 87 to
  88 in A legs.
- The rate also fell inside an A leg while its hold decayed (`01-A-133`: 55 to 61 fps for the first
  25 s at 52 ms, then 54, 57, 49, 54, 52 and 40 fps as the median fell to 16 ms), which suggested
  that the rate follows the latency.
- A later leg on the final build (early decode on, below) does not support that: it held 58 to 60 fps
  from 15 s to 50 s at 15 to 16 ms `decode_to_present_ms`, and showed 56.6 fps over the whole leg.
  The host's own fps in these runs moves between 38 and 60 within a leg, and the test client's window
  is on the display the host captures (one 5120x1440 display), so the capture is not independent of
  what the client draws. We did not isolate the cause of the lower rate in the B legs.

That first fix was reviewed and replaced by the clock rule below, and the repeat with the window out
of the capture is in the next section.

## The present clock rule, and the repeat with the client out of the capture

Review of the first fix found three problems in the clock (`protocol/core/src/present_clock.rs`):
the 25 us per frame fall was far too slow after any stall (a 50 ms delay takes about 33 s to fall at
60 fps), the 50 ms hold was a cap on the wait but not on the delay, and "the first 16
pictures" depended on the frame count, not on time (at 144 fps it is 111 ms, at 30 fps 530 ms). The
rule now:

- Fall: the gap between the held delay and the wanted one (the window's p97 plus 0.5 ms) closes by
  `dt / 500 ms` of the gap each frame, where `dt` is the host timestamp step clamped to 0 to
  100 ms, and never slower than `decay_us` (25 us) per frame. That is an exponential with a 500 ms
  time constant (the gap halves about every 350 ms of stream time), and a tiny gap still goes at
  25 us per frame.
- Cap: the delay never exceeds the window median plus `max_hold_us` (50 ms). One stalled frame cannot
  raise the delay for everything after it above what the stream usually needs.
- Warm-up: the first 250 ms of host time (a time, so the same at 30, 60 and 144 fps; `reset()`
  starts it again) show each picture at its ready time plus 0.5 ms and leave the window and the
  delay untouched.

Regression tests (`present_clock/tests.rs`): a stall in mid-stream does not hold the stream for
long; a stall of the same size every 64 frames is late only the first time; a 300 ms start burst at
144 fps is shown as it comes and then released; the existing slow-fall test now runs 300 plus 200
frames and checks the fall is at least 25 us per frame. The API is unchanged, so Android's engine
takes it as is.

The harness change that matters for the result: with one display, the client window sat inside the
host's WGC capture, so the client drew its own picture into the stream and the host's frame rate
followed what the client was doing. The first measurements in this file therefore had a feedback
loop (55 to 60 fps with the window visible; about 20 fps with it hidden and no animation). Both
`pyrowave-loopback.ps1` and the client now keep the window out of the capture
(`NIGHTJAR_EXCLUDE_FROM_CAPTURE=1`, `WDA_EXCLUDEFROMCAPTURE`). A first ABBA with that change streamed
about 20 fps because `pyrowave-anim.ps1` did not compile on PowerShell 7.4 (no animation, so the
host sent nothing new); it was discarded, the script fixed (longer `-ReferencedAssemblies`) and the
harness now fails the run when the animation process exits.

ABBA, 60 s legs, first 5 s dropped, early decode off, window out of the capture. A is the tree with
the first-16 rule and the single upload (`E-main.exe`: `fa35d03d` plus the capture exclusion), B has
the new clock too (`F-new.exe`). Times in ms; `frames` is the frames the host sent that reached the
decoder (session-end video line).

| Group | Leg | decode_to_present p50 / p95 | receive_to_present p50 / p95 / p99 | shown fps | frames |
| --- | --- | --- | --- | --- | --- |
| 133 | A 1 | 56.06 / 60.20 | 70.24 / 86.71 / 104.03 | 51.37 | 3013 |
| 133 | B 1 | 11.47 / 18.16 | 21.35 / 28.20 / 29.75 | 60.19 | 3581 |
| 133 | B 2 | 11.53 / 18.12 | 21.31 / 27.84 / 29.59 | 60.20 | 3585 |
| 133 | A 2 | 47.19 / 55.26 | 57.14 / 66.06 / 69.09 | 60.04 | 3582 |
| 133 | A pooled | 54.70 / 58.31 | 64.44 / 79.73 / 94.21 | 55.71 | |
| 133 | B pooled | 11.50 / 18.13 | 21.33 / 28.03 / 29.66 | 60.20 | |
| 300 | A 1 | 54.75 / 56.60 | 70.47 / 283.61 / 310.85 | 57.67 | 3444 |
| 300 | B 1 | 13.69 / 33.44 | 28.55 / 49.46 / 55.36 | 60.19 | 3587 |
| 300 | B 2 | 12.57 / 22.74 | 27.27 / 39.30 / 54.43 | 60.17 | 3587 |
| 300 | A 2 | 31.42 / 55.13 | 46.47 / 70.26 / 73.45 | 60.12 | 3587 |
| 300 | A pooled | 54.11 / 56.10 | 67.81 / 140.00 / 309.03 | 58.90 | |
| 300 | B pooled | 13.13 / 31.01 | 27.90 / 47.12 / 55.12 | 60.18 | |

The summary script says **KEEP** at both bitrates: both B legs beat both A legs on the
`decode_to_present_ms` p50, the pooled p95 is better, `receive_to_present_ms` p95 is better by 52 ms
at 133 and 93 ms at 300, and shown fps is higher (60.2 against 55.7 and 58.9), not lower. The host's
`sent_mbps` medians were 140 in the 133 legs (112 in the first, cold A leg) and 316 to 317 in the 300
legs, equal in both builds.

What this settles and what it does not:

- The fps failure of the first fix is gone, and we think it was the capture loop: with the window out
  of the capture B holds 60.2 fps, and the A legs are the ones that vary (shown fps 51.4 in the cold
  first leg; `receive_to_present_ms` p95 284 ms in leg 05 with `queue_wait_ms` p95 217 ms, which is a
  stretch of the client not drawing, not the clock). We did not find what stalled leg 05.
- A is also unstable in the clock sense: the old rule's delay falls 1.5 ms per second, so
  `decode_to_present_ms` depends on how long ago the last bad stretch was (56 ms in leg 01, 47 ms in
  leg 04, 31 ms in leg 08, the same build). B is 11.5 to 13.7 ms in all four legs.
- B's p95 at 300 Mbps (22.7 to 33.4 ms) shows the clock still raising the delay for the late frames
  of that bitrate (entropy decode p95 8.7 to 8.9 ms); that is what it is for.
- HEVC 1080p60 at the default bitrate (`-Codec Hevc`, one leg each way, order A B, so this is a
  check that the clock does not hurt, not a keep decision; host `sent_mbps` 18.5 in both):

  | Leg | decode_to_present p50 / p95 | receive_to_present p50 / p95 / p99 | shown fps | frames |
  | --- | --- | --- | --- | --- |
  | A | 8.54 / 17.78 | 8.83 / 18.08 / 25.09 | 60.11 | 3587 |
  | B | 8.11 / 13.94 | 8.40 / 14.25 / 15.75 | 60.21 | 3592 |

  Neither leg shows the 50 ms start hold (decode_to_present p50 is 8 ms in both); B's tail is
  shorter. H.264 was not measured, and neither was a stream started from the GUI.
- Four legs per bitrate on one PC are a small sample, and the A legs differ among themselves by more
  than some of the effects above; the verdict rests on the B legs being the same in all four.

## Ignored GPU tests

Run serially (`-- --ignored --test-threads=1`; parallel runs race on SDL init and quit) on the
final tree, under the GPU lock:

- `gpu_wavelets_match_reference_planes`: passes, max error 0.00193, rms 0.00072 (444=false,
  ten=false). The same figures as before the presenter changes: the fixes do not change pixels.
- `single_upload_draws_the_same_pixels_as_uploading_both_textures`: passes, every pixel equal.
- `gpu_wavelet_1080p_latency`: 111.80 ms/frame at 1080p 4:2:0 and 139.26 ms/frame at 4:4:4, from a
  debug test binary with an empty entropy stage (allocation, upload, wavelets and a synchronous
  readback). A debug build is not a measurement of the release path, so these numbers are only a
  smoke check. An earlier run of the same test on the tree before the presenter changes, made by
  accident while another agent was using the GPU, gave 118 ms/frame; there is no valid before and
  after for this test.

## CPU cost of a real frame

`cpu_costs_of_a_pyrowave_frame` (release build) on the 120 envelopes dumped from a 300 Mbps
baseline leg (146988 bytes per frame, 1920x1080, 4:2:0), run on this PC alone under the lock:

| Stage | Offline, ms p50/p95 over 360 frames | In the stream (baseline 300 leg) |
| --- | --- | --- |
| assemble | 0.06 / 0.07 | 0.02 / 0.07 |
| clear | 1.15 / 1.29 | 0.98 / 1.64 |
| entropy | 1.87 / 3.51 | 3.02 / 6.93 |
| pack | 2.95 / 7.67 | 5.97 / 13.77 |
| alloc | 0.01 / 0.02 | 0.02 / 0.04 |

Decoder total 7.48 ms median, 15.05 ms p95 per frame offline. The single-shot timers print 7.94 ms
per MB of entropy-decoded blocks (289 blocks, 72348 bytes), 0.90 ms for the plane clear and 3.35 ms
for the half conversion plus frame allocation.

Entropy and pack cost more in the stream than offline (entropy about 1.2 ms, pack about 3 ms at
p50), with the host encoding on the same GPU and CPU and the client rendering in parallel. The
offline figure is the cost of the code alone. The extra in the stream is probably contention with the
encoding host on the same PC, but we did not test that.

## Early decode, one confirming leg

One leg, 300 Mbps, final build (`D-final.exe`, which also has the present clock change and the
launcher fix, no retry workaround), `NIGHTJAR_EARLY_DECODE=1`:

- Session end: `early decode: 0 frames decoded ahead, 0 fell back to the whole frame, 0 chunks
  dropped, 0.0 ms of decoding done while frames arrived`. The harness host profile does not turn
  on slice delivery (`video_slice_delivery`), so the host sends whole frames and early decode has
  nothing to start on. This leg therefore is not a test of early decode; a run with slice delivery
  on is still to do.
- `input_to_output_ms` p50/p95/p99 8.49 / 15.49 / 21.26, `receive_to_input_ms` p95/p99 2.36 / 9.11,
  `queue_wait_ms` p95/p99 1.80 / 8.56, `decode_to_present_ms` p50/p95 16.59 / 24.63,
  `receive_to_present_ms` p50/p95/p99 25.95 / 33.72 / 37.61, shown fps 56.56. The first 15 s sat higher
  (31, 25, 20 ms in 5 s buckets) before settling at 15 to 16 ms.
- The autostream launched on the first try without the retry.

## Notes for the next brief

- Dropped first launches were fixed upstream (`162dd027`, `8dec231f`); the retry workaround in the
  autostream was reverted. When the app list fails or the app is missing, the client now says so
  (`LaunchApp::Failed`, `LaunchApp::Missing`) instead of waiting.
- `PyrowaveStages` in the presenter has to survive `destroy` (test
  `the_pyrowave_stage_timers_survive_destroying_the_texture`), otherwise the session-end line
  never prints.
- Largest measured stage costs, in order: pack 5.0 ms, upload 3.5 ms (now 1.8), entropy 2.5 ms,
  clear 0.9 ms (p50, 133 setting). Passes are 0.1 ms CPU submit; their GPU time is not measured
  here.
