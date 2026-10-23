# B2 step 2: send each AMF slice as it arrives

Design for the host half of B2 step 2 in [rubylight-3.0-plan.md](rubylight-3.0-plan.md). Status: design, not implemented.
Step 1 passed (`rust/release/fixtures/amf-unit-probe-7900xt.json`): at 4K60 on the RX 7900 XT, AMF returns 4 or 8
HEVC or H.264 slices per frame, the first 3.6 to 4.5 ms before the whole frame would arrive in frame mode.

Goal: start sending a P-frame when its first slice comes out of `QueryOutput`, not after the last. Each slice (or group
of slices) becomes one of Moonlight's FEC blocks, with its own parity. No client change, no `PROTOCOL_VERSION` change;
stock Moonlight and Nightjar receive a normal multi-block frame. The client still decodes the whole frame (Nightjar's
partial MediaCodec input is a separate client brief), so the only gain is network time overlapped with encode time.

Recommendation: **go**, behind `video_slice_delivery = off` by default, H.264 and HEVC P-frames only. AV1 is out
(tile frames were 2.6x to 4.7x the target size in step 1, and the AV1 depacketizer uses `lastPayloadLen`).

## 1. Wire format

Every video packet today (`rust/core/src/packet.rs`, `frame_blocks`): RTP header, then the NV header with
`flags` (byte 24: `1` PIC_DATA, `4` SOF on packet 0 of the frame, `2` EOF on the last), `multiFecFlags = 0x10`,
`multiFecBlocks` (byte 27: `block << 4 | (blocks - 1) << 6`), `fecInfo = index << 12 | dataShards << 22 | pct << 4`,
`streamPacketIndex = seq << 8`, `frameIndex`. Block 0's first shard starts with the 8-byte frame header:
`[0] = 1`, `[1..3]` host latency in 0.1 ms, `[3]` frame type (1 P, 2 IDR, 5 after invalidation),
`[4..6]` last packet payload length, `[6..8]` critical flags.

What each field needs, and what we know when slice 1 arrives:

| Field | Depends on whole frame? | Streamed value |
|---|---|---|
| frame type `[3]` | no, known at submit | always 1: IDR and after-invalidation frames are never streamed |
| latency `[1..3]` | today capture-to-packetize of the whole frame | capture to first unit polled; documented, it reads ~3 ms lower |
| last payload length `[4..6]` | yes | write the full shard size `slice`. Both clients ignore it for H.264/HEVC (common-c `VideoDepacketizer.c` ~908 uses it only for non-H.264/HEVC; Nightjar `depacketizer.rs` only for `Codec::Opaque`) |
| block count (byte 27 bits 6-7) | yes, must be fixed at block 0 | fixed per streamed frame, chosen before slice 1 (section 2) |
| block number, per-block `dataShards`, `pct` | per block | each block's own values |
| SOF/EOF flags, frame index, RTP timestamp | no / last packet only | EOF set on the last packet of the last block, same as today |
| total size | yes | not on the wire |

Receivers, checked:

- moonlight-common-c `RtpVideoQueue.c` `RtpvAddPacket` (~543-804): reads `bufferDataPackets` and `fecPercentage` from
  the first packet of *each block* (~703-709), so per-block shard counts and percentages may differ (we already send
  differing last blocks). `multiFecLastBlockNumber` must be the same on every packet of a frame (~730), hence the fixed
  block count. Blocks must arrive in order; a block that cannot be recovered drops the frame and calls
  `notifyFrameLost` (~595-662), as today. Every non-EOF data packet must be a full shard (~732); only the frame's
  final shard may be short, zero padded at the tail (~440-443).
- Nightjar `protocol/client/src/video/queue.rs` `add_ordinary` (~366-440) and `depacketizer.rs` (~446-519): same rules.
- `isFirstPacket` masks PIC_DATA, so our first-packet flags `0x05` stay valid.

So the only new constraint is the data layout: a block may not end on a short shard. Blocks are contiguous shard
ranges of the frame's byte stream; a slice's tail that does not fill a shard is **carried** into the next block, never
padded. The frame on the wire is then byte-identical to today's except for where the block boundaries fall, and
mid-frame zero padding never happens.

## 2. Limits and the block plan

Protocol limits (all enforced in `frame_blocks` today): at most 4 blocks; `data + parity <= 255` per block when
`pct > 0`; up to 1023 data shards per block at 0%; at most 4092 shards per frame. `slice = packet_size + 16 - 32`
(1376 at the default packet size).

Plan per streamed frame, `B` = configured blocks (2 or 4), `S` = slices requested from AMF:

1. Eligible only if: `Encoded::frame_cap_bytes` is `Some(cap)` (AMF `MaxAUSize`, auto = 2x per-frame budget), the frame
   is a P-frame, and `ceil(cap / slice) <= B * 1023`. Otherwise the frame goes the whole-frame way (section 5).
2. Block `k < B - 1` is emitted when unit `ceil((k + 1) * S / B)` has arrived (with `S = B`, one slice per block).
   It contains every complete buffered shard minus a hold-back, and never more than `255 - parity(k)` shards at the
   frame's FEC percentage (the rest is carried).
3. Hold-back, so the frame can always finish: keep at least `B - k - 1` complete shards buffered (each later block
   needs at least one), and emit only if `ceil((cap - emitted_bytes) / slice) <= (B - k - 1) * 1023`, i.e. the worst
   case remainder still fits the remaining blocks at 0%. If either fails, the unit just stays buffered.
4. On the last unit, the remainder is split over the remaining blocks with the same fitting as `conventional_fec`:
   full percentage if it fits 255, otherwise lower the percentage per block, down to 0% at 1023 data shards
   (`fec_limited` counts it, as for whole frames today).
5. If fewer than `B - k` shards exist at the end (tiny frame that still got streamed), impossible by rule 3.

At 4K60 HEVC 80 Mbps a frame is ~167 KB (~122 shards), the cap ~243 shards: all 4 blocks fit at full FEC. At 500 Mbps
HEVC the cap is ~1515 shards: rule 3 holds back until the remainder fits two blocks at 0%, so at most 2 blocks go
early and the tail loses FEC only when the frame really is that big, as whole frames do today.

Overflow (a unit past `cap`, which `MaxAUSize` should make impossible): the frame cannot be completed in the
announced block count. Send what fits, then log `warn!` once per session, bump a `slice_delivery_overflow` counter in
the stream stats, request a recovery frame through the existing `EncoderRecovery` path, and disable slice delivery
for the rest of the session. Loud, never a silently broken stream. Unit test it with a fake cap.

## 3. Pacing, loss recovery, timing

- Pacing (`rust/core/src/network_pacing.rs`, `rust/host/src/video_send/transport.rs`): unchanged. Each block goes
  through the same `Pacer` at the same rate (wired 800 Mbps, Wi-Fi 2x stream bitrate, at least 1.25x needed; loopback
  `Link::default()`). The pacer has no catch-up burst, so idle time between blocks is not paid back as a burst.
- Wi-Fi: this is where the gain is largest, because the paced send of a whole frame is long (section 7).
- Loss: `SendLoss::new(block_layout)` becomes incremental: `push_block(data, parity)` per sent block, same
  `unrecoverable()` rule. Smaller blocks at the same percentage recover less from bursty loss; this is the 1% gate.
- RFI and keyframes: IDR and after-invalidation frames are whole-frame, so `keyframe_loss` pacing and
  `recovery_progress` in `transport.rs` are untouched. A started frame cannot be skipped by `skip_pending`; if the
  stream abandons it (shutdown, encoder reset), later units of that frame are dropped and its frame index stays
  consumed, as `abandoning_conventional_blocks_keeps_frame_and_nonce_progress` already requires.
- LTR: references are per frame, unchanged. Verify in 2f that `h264_reference_recovery` still sees one picture.
- Intra refresh: off in v1 when slice delivery is on (refresh stripes and slices interact); revisit with data.
- A8 (`rust/core/src/performance.rs`, `docs/timing-record.md`): `Claim::sent(encoded, packetized, first_sent,
  last_sent)` gets `encoded` = poll time of the last unit, `first_sent` = first packet of block 0, so `first_sent`
  can precede `encoded`. Stamps keep clamping stages to zero. Append two columns to the host record, after
  `last_sent_us`: `first_unit_us` (poll time of unit 1, empty when not streamed) and `early_blocks` (blocks sent before
  the last unit). `timing_report.py` gets an `overlap` stage = `encoded - first_sent` for streamed frames and reports
  `packetize_pace` from `first_sent` when present. The 0x5536 `HostTimings` message is unchanged.

## 4. Slices and quality

Today `rust/core/src/encoder_policy.rs` (~476-484) sets `SlicesPerFrame`/`HevcSlicesPerFrame` to the client's RTSP
request, best effort, and output mode stays frame. `rust/PERFORMANCE.md` (~3420-3510) shows slices cost no encode time
or bytes at our settings, but there is **no quality data**. Slices break intra prediction and entropy context at
slice edges, so expect a small SSIM loss. With slice delivery on, set slices to `max(client request, B)` and
`OutputMode`/`HevcOutputMode = 1`. 2f measures SSIM with `rust/release/amf_sweep.py` (slices 1 vs B at fixed bitrate);
more than 0.002 SSIM loss is a kill (section 8).

## 5. Encoder output and threading

- AMF (`rust/windows/src/amf.rs` `poll`, ~1433-1539): one `QueryOutput` per call, `QUERY_TIMEOUT` 1 ms. In slice mode
  each call returns a unit; `OutputBufferType` 1 = slice, 2 = last slice (`log_unit` ~1546 already reads it). Keep the
  `in_flight` entry until the last unit; read `OutputDataType` (picture type) and `output_is_idr` from the first unit.
- `rust/windows/src/encoder.rs` `Encoded` gets `unit: Unit` with `Whole`, `Slice { index }`, `Last { index }`. When
  slice delivery is off, AMF stays in frame output mode and every `Encoded` is `Whole`: the default path does not change.
- Ineligible frames in slice mode (IDR, after invalidation, unknown cap, session disabled after overflow) are
  concatenated in `amf.rs` and returned as one `Whole` `Encoded` at the last unit: same latency as frame mode
  (the probe showed the full frame is no slower), no new hop.
- Inline path (`rust/host/src/stream.rs` `send_frames`, ~1573-1830): today it runs `conventional_blocks` and sends after
  each `Encoded`. Streamed units feed a per-frame `StreamedFrame` and each ready block is encrypted and sent in the same
  call. No new thread, no channel, same 100 us `OUTPUT_POLL` loop (~1990-2160).
- Threaded path (`rust/host/src/video_send.rs`, used when paced, large, automatic or keyframe-loss): units go over the
  existing channel to the packetizer thread, which owns the `StreamedFrame` and hands each block to `transport.rs`.
  Same two hops per unit as a whole frame has today. `State`'s single pending slot gains a "streaming" state that
  `skip_pending` and `claim_backlog` must not drop; a newer frame waits until the streamed one has its last block.

Core code (`rust/core/src/packet.rs`): factor the per-block body of `frame_blocks` into `fill_block(block, first_shard,
shards, pct, blocks_total, last)` and build both `conventional_blocks` and the new
`VideoPacketizer::streamed(timestamp, latency_us, blocks, cap) -> StreamedFrame` on it, with
`StreamedFrame::push(&[u8], last: bool, FnMut(&mut PacketBlock) -> Result<usize>)`. `block_layout` gets a streamed
counterpart for `SendLoss`.

## 6. Config and measurement gate

Config: `video_slice_delivery = off | 2 | 4` (blocks), default `off`, in `docs/configuration.md`, not in the console
until it passes the gate. Off means frame output mode and the exact current code path.

Tests (each brief adds its own; every one fails on today's code or guards a fallback):

- `packet/tests.rs`: streamed frame with one slice per block equals `conventional_blocks` bytes when the block
  boundaries match; carry across blocks keeps every non-EOF packet full size; per-block `dataShards`/`pct` and fixed
  block count on every packet; hold-back with a tiny frame; cap rule at a 500 Mbps-sized cap; FEC recovery per block
  using the existing gf helpers; overflow returns an error and keeps frame/nonce progress.
- Decode check: a test reassembles streamed packets with the same rules as `RtpVideoQueue.c` and compares bytes;
  2f runs the real C receiver and stock Moonlight.
- `amf.rs`: unit sequencing with a fake output (slice, slice, last), concatenation for ineligible frames.
- `stream_policy.rs`: incremental `SendLoss` matches whole-frame `SendLoss` on the same layout.
- `video_send.rs`: a streamed frame is not skipped by a newer pending frame.

Loss: add `BUTTERPOLLO_TEST_SEND_LOSS=percent:seed[:air]` beside `SendOutage` in `rust/core/src/stream_policy.rs`
(seeded, per datagram, applied where `SendOutage` is in `rust/windows/src/net.rs`), with parse tests.

Gate runs, release build, A8 records (`BUTTERPOLLO_TIMING_RECORD`, `rust/release/e2e.py --record`, then
`rust/release/timing_report.py`), A/B with `off` vs `4`, same seed, at least 3000 frames each:

1. Loopback C receiver (`rust/tests/moonlight_client.c` via `e2e.py`): 4K60 HEVC 80 Mbps and H.264, 0% and 1% loss.
2. Paced Wi-Fi emulation on loopback: `pacing_max_bitrate_kbps=160000`.
3. NUC over Ethernet (`ssh nuc`), then a phone on Wi-Fi with Nightjar and stock Moonlight (user runs the phone).

Pass: median `end_to_end` (picture age) improves by at least 1 ms in run 2 and run 3's Wi-Fi case; at 1% loss the
frame-loss rate and p99 `end_to_end` are no worse than `off` within run-to-run noise; zero `slice_delivery_overflow`;
no decoder errors in either client.

## 7. Expected gain

With `W` = paced send time of the whole frame, `E - E1` = time from first unit to last unit, `N` blocks: the frame
finishes `min(W * (N - 1) / N, E - E1)` sooner.

- Loopback or wired 800 Mbps, 4K60 HEVC 80 Mbps (~200 KB with FEC, W ~2 ms): ~1.5 ms.
- Wi-Fi at 160 Mbps pace, same stream (W ~10 ms): ~3.6 ms, bounded by the slice lead.
- 1080p60 on Wi-Fi: ~1.3 ms. H.264 gains ~0.5 ms more (later last slice in step 1).

These are estimates from step 1 numbers, not measurements; 2f measures them.

## 8. Kill criteria

Stop and revert to `off`-only (remove the code) if any of these hold after 2f:

- median picture age improves by less than 1 ms on emulated and real Wi-Fi;
- 1% loss regresses frame loss or p99 picture age;
- SSIM drops by more than 0.002 with B slices;
- decoder errors or corruption on stock Moonlight or Nightjar;
- any frame exceeds the cap in a gate run;
- any AMD encoder hang (exit code 3 from `e2e.py`): stop, investigate, never retry.

## 9. Briefs (in order, one implementer each, ~100 turns)

- **2a core** (`rust/core/src/packet.rs`, `packet/tests.rs`, `stream_policy.rs`): `fill_block` refactor with
  byte-identical output (existing fingerprint tests must pass unchanged), `StreamedFrame`, streamed `block_layout`,
  incremental `SendLoss`, `BUTTERPOLLO_TEST_SEND_LOSS`. Check: `cargo test -p butterpollo-core`.
- **2b AMF** (`rust/windows/src/amf.rs`, `encoder.rs`, `encoder_policy.rs`, config): slice output mode when on,
  `Unit`, concatenation for ineligible frames, intra refresh off. Check: unit tests plus `amf_unit_probe` unchanged.
- **2c inline send** (`rust/host/src/stream.rs`): streamed frames on the inline path, overflow handling and counter.
  Check: host tests, loopback `e2e.py` with `video_slice_delivery = 4`.
- **2d threaded send** (`video_send.rs`, `video_send/transport.rs`): units through the packetizer thread, streaming
  state in `State`. Check: video_send tests, paced loopback run.
- **2e timing** (`performance.rs`, `docs/timing-record.md`, `timing_report.py` and its tests): `first_unit_us`,
  `early_blocks`, `overlap` stage.
- **2f measurement** (runner, no code): section 6 runs and SSIM; result into `rust/PERFORMANCE.md` and the plan table.
