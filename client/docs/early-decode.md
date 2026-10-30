# Early decode

[Documentation](index.md) · [Beam racing](beam-racing.md) · [PyroWave GPU decoding](pyrowave-decode.md)

The Nightjar desktop client can decode the blocks of a PyroWave frame while the rest of the
frame is still arriving. It is the `early_decode` setting (off by default) or
`NIGHTJAR_EARLY_DECODE=1` for a measurement run. Nothing changes on the wire, so it works
against any host that sends PyroWave record framing, Vibepollo-compatible hosts included.

This document records what the bitstream allows, what was built, what it can and cannot gain
with the host's order today, how loss is handled, and how to measure it.

## What early decode can gain today: the entropy decode, not the picture

With the packet order the host sends today, early decode moves CPU work, not GPU work. It
cannot make a picture appear earlier, because nothing can be reconstructed before the last
record arrives. What it can do is take the entropy decode and the plane clear out of the time
after the last packet. The release CPU costs (measured on this PC, `cpu_costs_of_a_pyrowave_frame`):

| piece | cost | moved by early decode |
| --- | --- | --- |
| entropy decode | 24.6 ms per MB of blocks (289 blocks, 72 KB fixture) | yes, all but the tail records |
| plane clear (1080p) | 1.2 ms | yes |
| half conversion plus frame allocation (1080p) | 13.3 ms | no, needs the final planes |
| GPU inverse wavelet and present | not measured here | no |

The 24.6 ms per MB figure is a lower bound: the fixture is a 72 KB frame that stays in the
cache, and the figure excludes the plane stores a full 1080p frame makes (three f32 planes,
about 25 MB). A real frame costs more per byte.

### The decoder thread at high bitrates: model and measurement disagree

At 150 to 300 Mbps and 60 fps a frame is 312 to 625 KB. Taking the fixture costs above at face
value, the decoder thread would spend per frame:

| bitrate | entropy decode | plane clear | half conversion | total | frame period at 60 fps |
| --- | --- | --- | --- | --- | --- |
| 150 Mbps | 7.7 ms | 1.2 ms | 13.3 ms | 22.2 ms | 16.7 ms |
| 300 Mbps | 15.4 ms | 1.2 ms | 13.3 ms | 29.9 ms | 16.7 ms |

That predicts a saturated thread. The loopback run below says otherwise, so the model is too
pessimistic in absolute terms: the fixture is a 72 KB frame resident in the cache, and it
excludes the plane stores. Do not use the table for sizing; use the measured figures.

Measured on this PC (2026-10-23, early decode off, Nightjar desktop against an isolated host
on loopback, PyroWave 1080p60, about 60 s each, `NIGHTJAR_TIMING_CSV`). The host log shows the
frames at the bitrate budget (`sent_mbps` 100 to 150 at the default 133 Mbps, 315 to 319 at 300
Mbps). "Decode" is `input_to_output_ms`, decoder input to output.

| | 133 Mbps (default) | 300 Mbps |
| --- | --- | --- |
| decode median | 9.66 ms | 12.77 ms |
| decode p95 / p99 / max | 13.07 / 14.50 / 16.61 ms | 17.26 / 18.45 / 20.77 ms |
| frames over 16.7 ms | none | 8.6% |
| received / presented fps | 60.2 / 59.1 | 60.2 / 59.9 |
| decoder queue drops (bound 15) | 0 | 0 |

So the decoder keeps up on average at both bitrates and drops nothing, but at 300 Mbps the
median leaves about 4 ms of the 16.7 ms period and the p95 and p99 are above it. Bursts of
slow frames then back up the queue (`receive_to_input_ms` p95 82 ms, p99 175 ms at 300 Mbps),
which is added latency even though no frame is dropped. At 133 Mbps there is about 7 ms of
headroom. The measurement is not split into entropy decode, plane clear and conversion; it is
loopback, with the host encoder, a synthetic scrolling-texture animation (about two thirds of
the picture) and other sessions sharing the PC, so the backlog episodes at 300 Mbps may be
partly contention. It is one run per bitrate, not a statistic.

The gain needs decoder headroom: a decoder thread that is idle for part of each frame period.
Then early decode can use the idle time while the frame is still arriving. At most it moves the
entropy decode of the records that arrive before the last one, and no more than fits inside the
frame's time on the wire: at line rate that is 2.5 ms for a 312 KB frame and 5.0 ms for a 625 KB
frame at 1 GbE (half that at 2.5 GbE), a ceiling of 2.5 to 5 ms whatever the bitrate; host
pacing can spread a frame over longer and raise it. At 300 Mbps that ceiling is about the
headroom the decoder has left, so early decode could cut the tail; whether it does is for the
A8 run to say. The half conversion (about 13 ms by the fixture) stays after the last packet in
this step; it is the piece progressive conversion in steps 2 and 3 has to remove, and the piece
to make cheaper (on the GPU, or with a cheaper conversion) if the decoder is the limit. Nobody
should claim a gain before the A8 run says otherwise.

The reason is the bitstream, not the client.

### The PyroWave bitstream (186f0393)

A frame is an 8-byte sequence header followed by block records. A record is one 32x32
coefficient block: word 0 holds its size in words (bits 16-27) and the sequence (bits 28-30),
word 1 shifted right by 8 is the block index. Inside a block there are 16 8x8 sub-blocks with
ballot, bit-plane and sign coding. Decoding a block (`decode_block`) needs only its own bytes
and is independent of the sequence bits, so records decode in any order and a record can be
decoded the moment its bytes are complete.

The index space is ordered by wavelet level, coarse to fine: level 4 first (the 12 coarse
bands, which hold the picture at 1/32 size and are the part every row needs), then levels
3, 2, 1, 0, each over components 0..2 and bands, with the finest luma bands last. One band
covers the whole picture. So record index says what level a block is, and a block's position
in the picture is derived from the index within its band.

### What the host sends today (`rust/core/src/pyrowave.rs`, `record_frame` and `layout`)

The header, then the coarse records (the critical shards, in FEC block 0), then the fine
records in ascending index order, packed first-fit into packets of at most 1400 bytes. Up to
three further FEC blocks carry the detail. The inverse wavelet needs all levels of a region
before any output pixel of that region exists, and the coarse and fine data of one band cover
the whole picture. So until the last fine records arrive, no part of the picture can be
reconstructed. Nothing can be shown early with this order.

### Where the client waits today

1. `RtpQueue` holds packets until each FEC block completes (or the next block or frame
   starts), then moves them on.
2. The depacketizer builds the records envelope (`[0u32]` then `[len][flags][payload]` per
   packet) and hands the `Frame` over when the last block completes, or when the next frame
   starts if the last block was incomplete.
3. The decoder thread runs the CPU entropy decode of every record into three f32 planes
   (`coefficients`), converts the planes to an RGBA16F wavelet picture (`coefficient_frame`),
   and the presenter runs the inverse wavelet in SDL shader passes on the GPU.

Early decode takes the first half of item 3 out of the wait: the entropy decode and the plane
clear run during the frame's time on the wire. It cannot touch the half conversion (it writes
every row and the planes are final only after the last record) or the GPU passes (they need
every level). What is left after the last packet is the half conversion, the upload and the
GPU passes. The time early decode can save is therefore bounded by the CPU entropy decode plus the
plane clear, and only the part that fits inside the frame's time on the wire (table above).

What step 3 changes: records arrive in row order with the coarse data first, so the bottom
rows are the only part still missing late in the frame. Then the half conversion and the
inverse wavelet can run on row bands while the tail arrives, and the picture can be presented
(step 2) or raced (step 4) before the last packet. Those are the steps that remove the wire
time from the picture age. Step 1 is the receive-side plumbing they stand on: records leave
the receive path packet by packet, and the decoder takes them as they come.

## What was built

### nightjar-core

- `Records` (the PyroWave record reassembler in `protocol/core`) is now incremental:
  `begin_frame`, `feed_envelope` or `feed_packet`, `drain_new` (records completed since the
  last call) and `finish_frame`. `push_frame` is those three in sequence, so a frame fed whole
  and the same frame fed packet by packet give the same pushes, cache and loss figure. The
  tests compare them on random frames with random losses and feed sizes. The crate is
  `no_std` and shared with Android and the FFI, so the change is additive and the existing
  call is unchanged.
- `VideoConfig::early_chunks` and `VideoSink::chunk(first, flags, payload)`: with the option
  on, the receiver gives the sink each packet of the records envelope at the moment it is
  appended to the envelope, ahead of the finished `Frame`. The `Frame` and its envelope are
  byte-identical with the option on or off (tested on random loss and reordering, comparing
  frames, signals and statistics).
- `RtpQueue` appends the data packets of a FEC block that arrived in order, from the block's
  first packet up to the first hole, without waiting for the block to complete (they stay
  queued for FEC recovery). When the block completes, or is staged incomplete because the next
  block or frame began, the packets not yet appended follow in sequence order. A hole thus
  holds back the packets behind it until parity fills it or the block is given up, exactly as
  the envelope order requires.
- Session: `VideoDecoder::wants_early_chunks` and `VideoDecoder::chunk`. Chunks travel to the
  decoder thread in the same queue as the frames (frames and chunks keep their order), the
  chunk payload buffers are pooled, and a chunk over a fixed bound (16384) is dropped. Only
  the decoder thread path takes chunks (PyroWave never uses direct submit).

### nightjar-desktop

- `Settings::early_decode` (default false; a settings file without the key stays off) and the
  `NIGHTJAR_EARLY_DECODE` override. There is no toggle in the settings screen yet (the screen
  is being reworked elsewhere); it is a file or environment setting for now.
- `stream::pyrowave::Decoder::chunk` feeds `Records` and decodes each record as it completes
  into the planes (with a first chunk the planes are cleared and the frame state resets).
  `Decoder::coefficients` then decides: if the chunks add up to exactly this frame's envelope
  (checked by byte count) it finishes the frame in `Records` and decodes only the records the
  chunks did not give, which are the ones carried over from the previous frame to cover lost
  records. Otherwise (a chunk was dropped, the frame was flushed, a chunk did not parse) it
  throws the early work away and decodes the frame whole, from the envelope, the way it
  always did, and counts a fallback. The frame is the authority; chunks are a speculative
  head start.
- Counters (`Shared::counters`): `early_frames`, `early_fallbacks`, `early_us_total` (time
  inside `chunk`). A fallback is also printed once to stderr; it is not silent. A non-first
  chunk that arrives while the decoder has no frame start (its first chunk was dropped) marks
  the frame a fallback too.
- Visibility: with the setting on and PyroWave negotiated, the overlay shows an "Early decode"
  line (frames and fallbacks in the last second); at the end of the session the early frames,
  fallbacks, time inside `chunk` and chunks dropped over the queue bound are logged to stderr;
  and the timing CSV gains two columns after the host's, `early` (0 off, 1 early, 2 fallback)
  and `first_block_ns` (the session-clock time the first block of an early frame was decoded,
  0 otherwise), so the first packet to the first block decoded can be read per frame.

### Cost, honestly

- One copy per packet: the chunk payload goes from the receive path into a pooled buffer for
  the decoder thread (at most 1400 bytes per packet). It is a memcpy of the packet's payload
  beside the one the envelope already makes. No allocation per frame in the steady state
  (buffers and the envelope are pooled), and the planes are written once per block exactly as
  before.
- Nothing is added when the setting is off: no chunks are produced, the queue and the
  envelope take the old code paths, and `Decoder::coefficients` runs the whole-frame path.
  Tests assert this (frames, signals and statistics identical, and no chunk events).
- The planes are cleared when the frame's first chunk arrives instead of when the frame
  completes. The decoder thread is idle then in the common case. If the decoder is still on
  the previous frame, the chunks queue behind it and catch up.
- `decode_us_total` and the overlay's decode time count only what is left in `submit`, which
  shrinks with early decode. Compare picture age (the A8 records), not the overlay's decode
  figure, between runs with it on and off.

## Loss: a missing block never breaks the frame

- The early path does not change what is delivered. The `Frame` envelope is the same with
  and without it, and the final `Records::finish_frame` is the same call. Anything the early
  path decodes is a record `finish_frame` would push, and the decoder skips those when it
  walks the frame (`seen`), so the picture is the whole-frame picture. The tests compare
  planes from chunk-by-chunk decoding with whole-frame decoding, including frames with lost
  packets whose missing records come from the previous frame.
- A lost packet leaves a hole: the packets behind it wait for FEC recovery or for the block to
  be given up, then follow. A record cut by the loss is not decoded, and `Records` carries the
  previous frame's record over, as it does today. A late tail blurs the picture and never
  corrupts it, because a record is decoded whole or not at all.
- Chunks the decoder never gets (queue bound, `flush_frames` after a decoder refresh) leave
  the byte count short of the frame's envelope. The frame then decodes whole and the fallback
  is counted.
- A frame that fails to decode fails the same way with and without early decode (tested for a
  sequence header that names another picture).

## Measuring

All of this is for the gate in the plan: the NUC (`ssh nuc`, wired), Nightjar desktop, a host
on this PC, with A8 records on both sides. Do the idle check first
(`(Invoke-WebRequest http://127.0.0.1:47989/serverinfo).Content` must show
`RustHostSessionCount` and `RustHostPendingSessionCount` at 0) and take
`C:\src\butterpollo-release\GPU-LOCK`.

1. Build a release Nightjar desktop on this PC and copy it to the NUC (the same binary for
   both runs).
2. On the NUC, for each of `NIGHTJAR_EARLY_DECODE=0` and `=1`, at the recommended PyroWave
   bitrate and at the link's maximum, on 1 GbE (and 2.5 GbE if the NUC link allows it), run
   a stream for 60 s at 1080p60 with `NIGHTJAR_TIMING_CSV=<file>` set. On the host set
   `BUTTERPOLLO_TIMING_RECORD=<file>`.
3. Join the two records with `rust/release/timing_report.py` and compare median and p95
   picture age, and the decode start (`first_block_ns` against the first packet's time).
4. Repeat with 0.1% and 1% loss (the loss gate), and check `early_fallbacks` stays at 0 on a
   clean link.
5. Revert the feature, or leave it off, if the median picture age is not better, or if p95
   or loss behaviour is worse. The analysis above bounds the gain by the entropy decode that
   fits inside the wire time, and says the picture itself still waits for the last packet,
   so expect a small gain at most with today's order; the first run is also a check that it
   costs nothing.

Without a link, `cargo test -p nightjar-desktop --release --bin nightjar -- --ignored
--nocapture cpu_costs_of_a_pyrowave_frame` prints the CPU pieces early decode moves: entropy
decode time per megabyte of blocks, the plane clear and the half conversion of a 1080p frame.

## Step 3 and 4, built

The host's row order (`pyrowaveRowOrder`) and the client's slice decodes are in: with the
`beam_race` setting on, the first quarter of a 1080p frame is decoded 1.65 ms (p50) after its first
packet while the frame takes 6.48 ms to arrive (loopback, 399 Mbps, 3589 frames, all row ordered).
[beam-racing.md](beam-racing.md) has the design, the measurements and what is still open. The list
below is the plan as it stood before that.

## Next steps (not part of this change)

- Step 3 on the host: send rows with their coarse data first behind a new RTSP hint
  (`x-ss-video[0].pyrowaveRowOrder:1`, host side done; the client does not send it yet). With
  it the chunk stream is in picture order and the decoder can work on row bands. The order
  rests on one band (64 picture rows) of lookahead: block row r of wavelet level L goes with
  band `r * 2^L - 1`, level 4 first. `a_band_decodes_exactly_from_the_blocks_a_row_order_sends_before_its_cut`
  checks it on the CPU decoder: with only the blocks sent before band k's cut, band k comes out
  bit-identical (maximum error 0.0 in every band) at 1920x1080 4:2:0, 1280x720 4:4:4 and two
  odd sizes; with no lookahead the error is 0.25 to 1.3 on the random coefficients, which
  range -0.5 to 0.5. The `--ignored` variant
  `a_real_frame_decodes_its_bands_exactly_from_the_blocks_before_their_cut` does the same on
  real 1080p dump frames (`NIGHTJAR_PYROWAVE_FRAMES`): 0.0 with one band, up to 0.18 coefficient
  units without.
- A row-banded half conversion and inverse wavelet in the presenter, so the work after the
  last packet shrinks to the last band.
- A settings toggle, once the settings screen rework lands.
