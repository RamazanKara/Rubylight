# Early decode (Rubylight 3.0 plan, B10 step 1)

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

At 150 to 300 Mbps and 60 fps a frame is 310 to 620 KB, so the entropy decode is roughly 8 to
15 ms of single-thread CPU time (extrapolated from the fixture, to be confirmed on the NUC,
which is slower than this PC). The ceiling for step 1 is that time minus the decode of the
records that arrive last, and it needs the wire time of the frame (3 to 8 ms at 1 GbE for a
frame of that size) to hide it. The half conversion (13 ms) is larger than the entropy decode
and stays after the last packet in this step; that is the piece progressive conversion in
steps 2 and 3 has to remove. This is an estimate until the A8 run says otherwise. Nobody
should claim a number before that run.

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
  inside `chunk`). A fallback is also printed once to stderr; it is not silent.

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
   picture age, and the decode-start column (the first packet to the first block decoded).
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

## Next steps (not part of this change)

- Step 3 on the host: send rows with their coarse data first behind a new RTSP hint. With it
  the chunk stream is in picture order and the decoder can work on row bands.
- A row-banded half conversion and inverse wavelet in the presenter, so the work after the
  last packet shrinks to the last band.
- A settings toggle (once the settings screen rework lands) and an overlay line with the
  early/fallback counters.
