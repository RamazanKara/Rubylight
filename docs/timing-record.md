# Timing record (3.0 A8, measure only)

Design and implementer briefs for [plan](rubylight-3.0-plan.md) item A8. Nothing here changes how a frame is captured, encoded, sent, decoded or shown. No `PROTOCOL_VERSION` change; stock Moonlight sees no difference.

## 1. What exists and what is missing

| Point | Where it is stamped today | Reaches the other side? | Gap |
| --- | --- | --- | --- |
| Game present | RTP timestamp from `present_timing::Stamper` (WGC + DXGI ETW refine), else the capture's present stamp (`video_send.rs` / `pyrowave_send.rs`, `stamp`) | Yes: every packet; common-c `presentationTimeUs` = `rtp × 1000 / 90` on the media clock | None. The host record needs its µs value. |
| Capture present | `Stamps::presented` (DDA LastPresentTime, WGC SystemRelativeTime, or arrival) | No | Absolute value not on the wire |
| Capture | `Stamps::acquired` | No (folded into 0x5536 `queue_us`) | Not split out |
| Hold/claim, convert | `published`, `claimed` | No (in `queue_us`) | Convert not split out |
| Encode submit | `Stamps::submitted` | As `present + queue_us` | Only relative to present |
| Encode complete | `Stamps::encoded` | As `+ encode_us` | Only relative |
| First / last packet sent | `first_sent`, `last_sent` | As `host_us`, `send_us` | Only relative |
| PyroWave, all host points | `pyrowave_send.rs` carries no `Claim` | **No 0x5536 records and no stage samples** | Missing |
| Client first packet | common-c / nightjar-core `receiveTimeUs` | Nightjar CSV `receive_ns` | None |
| Frame assembled | `enqueueTimeUs` | Nightjar CSV `enqueue_ns` | None |
| Decoder input / output | `MediaCodecDecoderRenderer` | CSV `decoder_input_ns`, `decoder_output_ns` | None |
| Present | `OnFrameRenderedListener` / release | CSV `render_ns`, `release_ns` | None |
| Clock offset | `present_clock::ClockOffset` fed by 0x5537 | Overlay "host to present" only | Probes run only with *host frame times* on; offset never exported |

So the client stages already exist (`FrameLatencyStats.CSV_HEADER`). The missing parts are: absolute host times per frame, capture and convert split out, PyroWave host stamps, clock sync independent of VRR presentation, the offset in the record, a host-side record for receivers that cannot speak 0x5536/0x5537, and the report script. 0x5536 and its extension are in [control-messages.md](control-messages.md#0x5536-host-timing).

## 2. Record design

### Clock domains

- **Host media clock**: µs since `media_epoch` (set when video starts, `stream.rs`). 0x5537 replies and RTP timestamps use it. All host columns are on this clock.
- **Client clock**: Nightjar uses `System.nanoTime()` (CLOCK_MONOTONIC). The C probe already stamps CLOCK_MONOTONIC (`android_control.c`). Brief B checks that the Rust engine's probe uses the same base.
- **Join**: `client = host + offset`, so host times map to `host_us + offset`. The report converts client times to the host clock.

### 0x5536 extension (rubylight-protocol, appended fields)

The v1 report has a header (`version, count, reserved, reserved`) followed by `count × 20` bytes. The current Rust and Java decoders check only `len ≥ 4 + count × 20` and ignore the reserved bytes. Extension:

- Header byte 2 is `extra`, the bytes per record in a trailing block. It is `0` in v1 and `16` now. The version stays `1`.
- After the v1 records, `count × extra` bytes follow. For each record: `presented_at_us` u64 (capture present on the media clock), `capture_us` u32 (presented to acquired), `convert_us` u32 (claimed to submitted). A decoder reads the first 16 bytes of each entry and skips the rest, so later fields can be appended.
- It is valid only if `capture_us + convert_us ≤ queue_us`. If not, the client drops the extension and keeps the base records.
- An extended report carries at most 17 records (616 bytes). The Rust and Java decoders accept counts up to 32 and ignore trailing bytes, but shipped Nightjar's patched common-c (`host-timing.patch`) hands 0x5536 to Java only if the message is at most 644 bytes, and `4 + 36 × N` exceeds that from N = 18. The cap constant lives in the protocol crate.
- The API is additive: `HostTimingExtra`, `HostTiming::encode_report_with(&[(HostTiming, HostTimingExtra)])` and `decode_report_with(&[u8]) -> Option<Vec<(HostTiming, Option<HostTimingExtra>)>>`. `encode_report`/`decode_report` are unchanged, so existing callers keep compiling.

The client derives these from the extension: capture present = `P`, acquired = `P + capture_us`, submit = `P + queue_us`, encoded = submit + `encode_us`, last sent = `P + host_us`, first sent = last sent − `send_us`. Hold + claim = `queue − capture − convert`, and packetize + pacing = `residual_us()`. Game present comes from the frame's RTP µs, which the client already has.

### Host-side record (opt-in)

Set `BUTTERPOLLO_TIMING_RECORD=<dir>` on the host process. It is unset by default and has no UI. While unset, no thread and no file exist. When set, each video stream writes `<dir>\host-<stream_id>.csv`:

```
# rubylight timing record 1
# media_epoch_qpc_ms=<QPC ms at media_epoch, as clock_ms() in moonlight_client.c>
# codec=<h264|hevc|av1|pyrowave> capture=<wgc|dda> present_source=<etw|capture>
wire_frame,rtp_us,present_us,acquired_us,published_us,claimed_us,submitted_us,encoded_us,packetized_us,prepared_us,first_sent_us,last_sent_us,first_unit_us,early_blocks
...
# dropped=<rows dropped because the writer fell behind>
```

All values are integer µs on the media clock. `first_unit_us` and `early_blocks` are empty unless the frame was streamed (below); records written before they existed end at `last_sent_us`, and the report reads both. The send thread does a non-blocking `try_send` into a bounded channel (4096 rows) at the point where `host_timings.record` is called today. A writer thread does the file IO. A full channel drops rows and counts them, and never blocks the send thread.

#### Streamed frames (`video_slice_delivery`)

A P-frame the host sends block by block while the encoder still produces it has its first blocks on the wire before its last slice exists. Its points are the same as for any frame, but `first_sent_us` can precede `encoded_us`:

- `encoded_us` is when the encoder's **last** slice was in the host's hands, as for a whole frame, so `encode` is the encoder's whole time. `first_unit_us` is when its **first** slice was, and `early_blocks` is how many FEC blocks were built before the last slice arrived.
- `packetized_us` is when packetizing began on the first slice, `prepared_us` equals `first_sent_us` (the first packet of block 0), and `last_sent_us` is the last packet of the **last** block.
- The host's own stages (`Stamps::stages`, the 0x5536 `encode_us` and `send_us`, the `stages` figures in the status JSON) count a point that falls before the one preceding it as reached with it, so no stage is negative and they still add up to present to last packet. For a streamed frame the encoder's time ends at `encoded_us`, `packetize`, `send_queue` and the start of `send` are zero, and `send` is the tail after the encoder: `last_sent_us` minus `encoded_us`. `Stamps::overlap()` is `encoded_us - first_sent_us`, the time the first block was on the wire before the encoder finished.
- `timing_report.py` reports, for a frame with `first_unit_us`: `encode` as above, `packetize_pace` from `first_unit_us` to `first_sent_us` (the first block's packetizing and pacing, inside `encode`), `overlap` = `encoded_us - first_sent_us` (floored at zero; zero when nothing left early), and `send` from the later of `first_sent_us` and `encoded_us` to `last_sent_us`. `packetize_pace` and `overlap` lie inside `encode`, so a streamed frame's stages are not a partition of `host_total`. The overlap row appears only when a streamed frame is in the record.
- Frames the host does not stream (keyframes, recovery frames, frames the plan cannot split) are joined and stamped as before.

### Client record

- **Nightjar latency CSV**: append these columns at the end and leave the existing ones and their order as they are: `rtp_host_us` (game present), `host_present_us`, `host_acquired_us`, `host_submitted_us`, `host_encoded_us`, `host_first_sent_us`, `host_last_sent_us`, `clock_offset_us`, `clock_rtt_us`. Host columns are µs on the host clock. A column is empty when its source is missing (no extension, report lost, or no 0x5537). The offset and RTT are read once per CSV flush, outside the stats lock, for the newest exchange. Drift within one flush is about 50 µs/s at most.
- **Test receivers** (`BUTTERPOLLO_TEST_TIMING_CSV`, `frames.csv`): `moonlight_client.c` stays as it is. nightjar-cli appends the same nine columns after its existing ones. `csv.DictReader` users are unaffected.

### Join and error bound

The report script picks the join mode per file and prints it next to every result.

1. **clock_sync** (Nightjar, nightjar-cli): per row, `offset = clock_offset_us`. `ClockOffset` uses the shortest-RTT exchange of the last ~8 s and carries it forward by the measured drift. Asymmetry error is at most `rtt_best / 2`. The host's receive and send stamps and any socket delay lie inside the round trip, so the bound holds. Granularity is 1 µs. The report prints `± rtt/2` (the session's median `clock_rtt_us / 2`) beside every cross-clock stage. Stages within one clock are exact.
2. **shared_clock** (loopback on this PC, C receiver): host and receiver both read QPC. The receiver's `first_packet_us` and `assembled_us` count from its own start (common-c `PltGetMicroseconds`), but `arrival_ms` is absolute QPC: exactly the assembly time with a hardware decoder, the start of decoding (assembly plus the software queue) otherwise. The report rebases with `base_us = min over rows of (arrival_ms × 1000 − assembled_us)` (the minimum strips the queue) and uses `offset = media_epoch_qpc_ms × 1000 − base_us`, with `client = host + offset` as in mode 1. The bound is 0 up to the clock's granularity. Rows are joined by `wire_frame`. This mode is also the reference for checking that mode 1 stays within its bound.
3. **min_owd** (C receiver on another machine, for example the NUC under Linux): `offset = min over frames (first_packet − first_sent)`. Cross-clock stages then show the one-way delay *above the minimum*, and end-to-end is understated by the minimum one-way delay. The report labels these as lower bounds.

### Stages reported (median, p95, p99, n, in ms)

Host: `game_to_present` (RTP to capture present, ETW only), `capture`, `hold_claim`, `convert`, `encode`, `packetize_pace` (encoded to first sent), `send` (first to last packet), `host_total` (present to last sent). Cross-clock (±bound): `network_first` (first sent to first packet; the host stamps "first sent" when the first send call returns, so on loopback, where the packet arrives during that call, it is a few tens of microseconds negative), `network_last` (last sent to assembled), `end_to_end` (game present to client present). Client: `assemble` (first packet to assembled), `client_queue` (assembled to decoder input), `decode`, `display` (decoder output to present), `client_total`. The report also counts host rows without a client row (lost or never shown) and client rows without host data (report lost).

Cadence (B1; each is one row of n, median, p95, p99, and is left out of the table when it has no pairs): the series are taken in frame-number order and each frame is paired with the previous one that has the stamp. `present_interval` is the gap between the game's present times of adjacent frames, the content's own cadence (the host record's rows, so it includes frames the client lost). `host_cadence_error` is `|(last_sent_i - last_sent_{i-1}) - (present_i - present_{i-1})|`, the cadence the host adds; it is judged against the present stamp, or the RTP stamp when a row has none, and uses the host record's rows when there is one, otherwise the client file's host columns. `client_present_interval` is the gap between `render_ns` of adjacent shown frames and `client_cadence_error` is `|render_ns gap - rtp_host_us gap|` over consecutive shown frames. `arrival_interval` and `arrival_cadence_error` are the same two against `enqueue_ns` (assembled, before the decoder); they need no display, so they also work for a frames.csv. A frame that never showed (lost, or dropped by the client) is skipped: the error of a pair that spans it is kept, since the shown gap should still equal the content's gap, but the interval is reported for adjacent frame numbers only. RTP stamps are differenced the short way round the 32-bit wrap. No clock offset is involved, so these rows have no bound. `--skip-first SECONDS` leaves the warm-up out of every row (the first frame whose host time is that late is the cutoff on both sides). `rust/release/cadence_matrix.py` runs the host's cadence across stream rates, codecs and the VRR hint with these rows. The percentiles are nearest-rank like the other rows, so p99 is meaningful from a few hundred pairs.

### What a moonlight-common-c receiver provides

Stock common-c has no API to send 0x5536 or 0x5537, so `rust/tests/moonlight_client.c` gets no host stamps over the wire. Its `frames.csv` already has `wire_frame`, `first_packet_us` and `assembled_us` (its own clock), `arrival_ms` (absolute QPC or CLOCK_MONOTONIC ms, about assembled), `decode_ms` (decode starts in the callback, so there is no separate queue), `render_qpc` (test renderer only) and `host_ms` (the frame-header latency). Combined with the host-side record it gives every host stage, plus `assemble` and `decode`. With the shared clock it also gives exact cross-clock stages, and with min_owd lower bounds. It does not give MediaCodec queueing or display. Nightjar's patched common-c engine and its Rust engine both do speak 0x5536/0x5537.

## 3. Briefs

Each brief runs in its own worktree. No `git stash`. LF endings. Commit messages go to a file (`git commit -F`). Push when the acceptance check passes and the reviewer has seen the diff.

### Brief A: protocol extension and host stamps (implementer, ~90 turns)

Owned: Nightjar `protocol/core/src/control.rs` (worktree from Nightjar `origin/main`). Rubylight `rust/core/src/host_timing.rs`, a new `rust/core/src/timing_record.rs` (pure row/format logic), `rust/core/Cargo.toml` (rev bump), `rust/host/src/stream.rs` (epoch QPC, sink creation), `rust/host/src/video_send/transport.rs`, the stamp site in `video_send.rs`, `rust/host/src/pyrowave_send.rs` and its `transport.rs`, `docs/control-messages.md`.

1. In Nightjar protocol, add `HostTimingExtra` and the additive encode/decode from §2, with the 17-record cap. Tests: round trip; the unchanged `decode_report` accepts an extended report with the same base records; an extension with `capture+convert > queue` is dropped and the base kept; a 616-byte cap (under the 644 of a full v1 report); trailing bytes beyond `extra=16` are skipped. Run `cargo test -p rubylight-protocol` in `protocol/`. Push to Nightjar main.
2. In Rubylight, bump the `rubylight-protocol` rev and `cargo update -p rubylight-protocol`. Read `git log <old>..<new> -- protocol/core` for anything else that changed.
3. `host_timing::timing` also returns the extra from `Stamps` plus the epoch. `HostTimings` stores pairs and `take_report` uses `encode_report_with`.
4. PyroWave: carry the `Claim` through `Frame`/`Prepared` to the PyroWave transport, and at the last packet call `record_stages`, `host_timings.record` and the record sink, as `video_send/transport.rs` does. First confirm from a host log that PyroWave `stage_frames` is 0 today.
5. Add the host-side record from §2. Keep the formatting in core (`timing_record.rs`), and put the channel and writer thread in host. Take the epoch QPC with `QueryPerformanceCounter` next to `Instant::now()` at `media_epoch`. Record `rtp_us` from the packetizer's `stamp`.
6. Document 0x5536 in `docs/control-messages.md`: a table row, a section with the request, v1 records and the extension, and the "17 records" rule.

Acceptance: `pwsh -File C:\src\rl-quick.ps1 -Repo <wt> -Crates butterpollo-core,butterpollo -TargetDir C:\src\rl-target-a8a` passes. New tests: extra values from fixed `Stamps`; a report with 24 extended records decodes in the old decoder; a PyroWave frame yields a 0x5536 record (pattern: `video_send/tests.rs:1171`); with the record sink unset no file is created; with it set the rows equal the `Stamps` points; with a stalled writer the send path returns at once and `dropped` counts. Report: commits on both repos, new rev, test names.

### Brief B: Nightjar client record (implementer, ~100 turns; after A step 1)

Owned (Nightjar worktree from `origin/main`): `HostTimingReport.java`, `FrameLatencyStats.java`, `HostFrameClock.java`, `MediaCodecDecoderRenderer.java` (CSV flush, probe start), `MoonBridge.java`/`RustBridge.java` (an offset getter), the native offset getter for both engines (`android_present_clock.c`, `protocol/android`), `protocol/android/src/listener.rs`, `protocol/cli/src/main.rs`, and their tests.

1. `HostTimingReport.parse` reads the extension, with the same validation as Rust and empty fields without it.
1a. Raise the 644-byte limit in `app/src/main/jni/moonlight-core/patches/host-timing.patch` (`length <= 644` for 0x5536). The host keeps extended reports at 17 records (616 bytes) so that shipped Nightjar builds still receive them, but the patch should accept the full extended size, `4 + 36 × 32`, so the cap can be raised later.
2. The Rust engine path must keep the extension. `listener.rs` re-encodes records with `encode_report` today, which would drop it. Forward the raw payload or re-encode with `encode_report_with`, and add a test.
3. Clock sync probing is gated only on the host announcing 0x5537 (`hostSupportsControlMessage`), not on `prefs.hostFrameTimes`. Scheduling (`presentNs`) stays behind host frame times.
4. Add `MoonBridge.presentClockOffset(long[] out)` returning `{offset_us, rtt_us}` (false before an exchange) for both engines. Check the client base is CLOCK_MONOTONIC on both.
5. Append the nine columns to `CSV_HEADER` and `writeCsv`. Read the offset once per flush, before the stats lock is taken.
6. nightjar-cli: when 0x5536/0x5537 are announced, request host timing, probe 4 times a second, and append the same nine columns to the timing CSV.

Acceptance: `.\gradlew.bat --no-daemon --max-workers=2 :app:testNonRootDebugUnitTest --tests '*HostTimingReportTest' --tests '*FrameLatencyStatsTest' --tests '*HostFrameClockTest'` passes while iterating, and the full `:app:testNonRootDebugUnitTest` plus `cargo test` for `protocol/android` and `protocol/cli` pass at the end. New tests:
- the header keeps its old prefix, byte for byte;
- the new columns are filled from an extended report plus an offset;
- they are empty for a v1 report or a host without 0x5537;
- probes start with host frame times off while `presentNs` returns `readyNs` unchanged;
- the Rust listener keeps the extension.

Host-neutral: gate only on HostCapabilities and the negotiated `x-rl-control` ids. Report: commit, test names, any base-clock finding.

### Brief C: report script and harness glue (implementer, ~60 turns; can run alongside A and B)

Owned: new `rust/release/timing_report.py` and `rust/release/test_timing_report.py` with small fixture CSVs under `rust/release/fixtures/timing/`, a `--record` flag in `rust/release/e2e.py` (sets `BUTTERPOLLO_TIMING_RECORD=<case>/record` for the isolated host and writes `timing` into `result.json`), and the CI step in `.github/workflows/rust-windows.yml` next to `test_release.py`.

1. Input adapters:
   - a Nightjar latency CSV;
   - `frames.csv` (C receiver or nightjar-cli) plus an optional host record.
   The join mode is chosen as in §2: clock columns present means clock_sync; a host record with QPC arrival means shared_clock; otherwise min_owd, or `--join` forces one.
2. Output: a stage table (median, p95, p99, n, ms; the bound beside cross-clock stages; join mode; unmatched counts) and `--json PATH`. Use `soak_result.percentile` (nearest-rank, floor) so the numbers match the other release reports.
3. With both a clock_sync file and the shared QPC available (loopback nightjar-cli), `--check-bound` fails if `|offset − shared offset| > rtt/2` on more than 1% of rows.

Acceptance: `python -m unittest discover -s rust/release -p test_timing_report.py -v` and the existing `test_release.py` pass. The fixtures cover all three join modes, missing columns, wrapped RTP, and unmatched rows. Report: files, sample table from a fixture.

### Brief D: baselines (runner, ~60 turns; after A–C are on main)

Do the idle check first. Stop if the user is gaming. Exit code 3 means an AMD encoder hang: stop and report, never retry. Use a release package from `build.ps1 -SkipTests -Package`.

1. Loopback on this PC: `e2e.py --record` with the C receiver for H.264, HEVC and AV1, and with the PyroWave receiver. Use 1920x1080x60, the e2e default bitrate, 60 s, and 3 runs each. Repeat H.264 with nightjar-cli plus `--check-bound`.
2. NUC (`ssh nuc`, Docker `butterpollo-test-client:20261005`, sources mounted as in `~/butterpollo-tests-20261015-pacing`): run the same matrix against an isolated host (`--config-dir C:\tests\a8 --port 48123 --bind <LAN IP>`, `virtual_display_layout = extended`). Use nightjar-cli where it builds there and the C receiver with min_owd otherwise.
3. Overhead A/B: H.264 loopback with and without `--record`. `present_to_sent_p99_ms` from the 5 s `stream timings` lines must not get worse beyond run-to-run noise.

Report: one stage table per codec and client, with the join mode and bound, plus the A/B numbers, raw file paths and anything that failed. Mark the phone runs (Nightjar MediaCodec stages) as pending for the user. Main session writes them into `rust/PERFORMANCE.md` and the plan status table.

## 4. Decisions

Decided 2026-10-22. Revisit if the user says so.

- Nightjar always sends clock sync probes when the host announces 0x5537 (4 packets of 16 bytes a second). No setting.
- The host-side record is an env var only (`BUTTERPOLLO_TIMING_RECORD`). It gets a config key only if a user-facing need comes up.
- On the NUC, if nightjar-cli doesn't build in the test image, those baselines report the network and end-to-end stages as lower bounds and say so.
