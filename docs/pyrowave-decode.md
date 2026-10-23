# PyroWave GPU decoding in the desktop client

Status: shipped in the stream (brief G2). The `pyrowave_decoder` setting picks `Cpu` (the Rust
decoder in `stream/pyrowave.rs`) or `Gpu` (the SDK, this document). Neither falls back to the other:
with `Gpu` and a failing probe, PyroWave is not offered and the settings screen says why (the probe's
error text, "Switch the PyroWave decoder to CPU ..."); with `Cpu` the DLL is never loaded.
The default is `Gpu` on Windows (every leg of the ABBA below keeps, on this PC and on the 780M
laptop) and `Cpu` elsewhere. A settings file that says `Cpu` keeps it.

## Route A: the SDK decodes, D3D11 presents

The PyroWave SDK (`libpyrowave-shared-1.dll`, API 1.0, built from the commit and patches in
`protocol/desktop/pyrowave-sdk.pin`) is loaded at run time with `libloading`. The Rust binding is
`protocol/desktop/src/stream/pyrowave_sdk.rs`:

- The SDK creates its own Vulkan device on the D3D11 adapter's LUID.
- The picture is three planes (Y, Cb, Cr; R8, or R16 for 10 bit) that are D3D11 textures created
  shared, then imported into Vulkan as `D3D11_TEXTURE` images. D3D11 samples the same memory the SDK
  writes: no copy on either side.
- Two shared `ID3D11Fence`s are imported as timeline semaphores. The decode fence: a decode signals
  `counter + 1` and the presenter queues `ID3D11DeviceContext4::Wait(fence, value)` before it
  samples the planes. The reuse fence: the presenter `Signal`s it after the draw that last read a
  plane set, and the next decode into that set waits for it on the GPU before it writes.
- A frame is `clear`, one `push_packet` per record (the sequence header record and the block
  records; the SDK accepts them in any order), `is_ready`, `decode(slot, after)` (the GPU waits for
  the reuse fence to reach `after`, acquires the planes from `QUEUE_FAMILY_IGNORED`, releases them to
  `QUEUE_FAMILY_EXTERNAL` and signals the decode fence), then the D3D11 wait. `wait(value)` is a
  CPU wait on the decode fence, for tests.
- Records come from `rubylight_protocol::pyrowave::Records`: `push_frame` for a whole frame, or
  `begin_frame` / `feed_packet` / `drain_new` / `finish_frame` to push records as they arrive.

### What the presenter must do (G2)

- **Reuse.** The D3D11 queue can run up to three frames behind the CPU, so a decode must not
  overwrite a plane set D3D11 has not finished reading. After each draw that samples slot `s`,
  `Signal(reuse_fence, n)` with a growing `n` (and flush, so the signal is queued before the next
  decode waits on it), remember `n` for `s`, and pass it as `after` to the next `decode(s, n)`.
  Pass 0 for a set nothing has read. Without it the decode would race the draw.
- **Stall guard.** `ID3D11DeviceContext4::Wait` has no timeout; if the Vulkan device hangs or is
  lost the decode fence never reaches its value, the D3D11 queue stalls behind the `Wait` and the
  display driver ends in a TDR. Call `GpuDecoder::ready_within(value, deadline)` first (it uses
  `GetCompletedValue` and a fence event, so a finished decode costs a read) and queue the `Wait`
  only on `Ok`. Its `Err` names the stalled value and the fence's position; report it loudly and
  drop the frame or the stream, never fall back to the CPU decoder silently.
- **Ownership.** `GpuDecoder` is `Send` but not `Sync`: move it between threads, never share it.
  The presenter must hold its own AddRef'd clones of the textures and fences it uses
  (`planes(slot)[i].clone()`, `fence().clone()`, `reuse_fence().clone()`) so a draw already queued
  keeps valid objects if the decoder is dropped on another thread.
- **Flush.** D3D11 frees released resources at the next flush of its immediate context; the
  presenter flushes every frame, and `GpuDecoder`'s Drop does not touch a context it does not own.
- Records come from `rubylight_protocol::pyrowave::Records`: `push_frame` for a whole frame, or
  `begin_frame` / `feed_packet` / `drain_new` / `finish_frame` to push records as they arrive.

### Probe and errors

`pyrowave_sdk::probe(device, ten_bit)` builds a minimal decoder, so every requirement is exercised
for real: with `ten_bit` the planes are R16, so a driver that cannot import them fails the probe
the stream asks for, as loudly as any other. The error names the first missing piece: the DLL
(path searched), its ABI (`found 2.1.4, this client needs major 1`), `ID3D11Device5` (shared
fences, Windows 10 1703), a Vulkan device for the adapter (with `diagnose_vulkan`'s account of the
Vulkan loader and the adapters it saw), the decoder itself (the same diagnosis names the Vulkan
1.3, subgroup size control, subgroup operation or interop extension the adapter lacks), the plane
format import, or either fence. These strings are unit tested with the DLL absent, with a DLL that
is not the SDK, with a fake DLL of the wrong ABI, and for the refused decoder.

The DLL is loaded once per process (`Api::shared`, never unloaded) and the probe result is kept per
adapter LUID and plane format, so a settings screen and a stream start ask for free. A load failure
is not remembered (the next ask looks again); a probe failure is, because the driver does not change
under a running process.

### D3D11 and handles

D3D11 frees a released resource at the next flush of its immediate context. The presenter flushes
every frame; a test that drops decoders on a long-lived device with no flush holds the plane
textures' and fence's handles (about 20 to 50 per life). `GpuDecoder`'s Drop releases everything it
made, in order (decoder, images, sync object, Vulkan device, then the D3D11 textures and fence) and
deliberately does not flush a context it does not own: the context is not thread safe and the
presenter's thread owns it.

## Building and packaging

`scripts/build-pyrowave-sdk.ps1` builds the pinned SDK with the Rubylight host's
`rust/tools/build_pyrowave.sh` (MSYS2 UCRT64) into `C:\src\butterpollo-release\nj-pyrowave-sdk`:
`libpyrowave-shared-1.dll`, `build-info.txt` (pin, patches, SHA-256, toolchain) and `licenses\`
(PyroWave, Granite, volk, and the notices of the MinGW runtime the DLL links statically: the
mingw-w64 winpthreads licence and the GCC runtime library exception). Packaging refuses a folder
without the winpthreads notice.

`protocol/desktop/package-windows.ps1 -PyrowaveSdkDir <that folder>` puts the DLL next to
`nightjar.exe` (with the DLLs it imports), its build-info as `pyrowave-sdk-build-info.txt` and the
licences in `licenses-pyrowave\`. It checks before the build that the folder matches the pin
(commit, Granite, API, patches) and that the DLL's SHA-256 is the one `build-info.txt` recorded, and
refuses otherwise. Without the switch the package has no GPU decoder.

## Measurements

RX 7900 XT, release build, one plane set, random but valid records filling a frame at about 0.3 of
its raw size (synthetic frames built from the fixtures' records), 300 decodes after 20 warm-ups. Run:
`cargo test -p nightjar-desktop --release --bin nightjar -- --ignored --nocapture
pyrowave_sdk::tests::decode_timing`.

| Size | Frame bytes | CPU push + submit p50 / p95 | Start of push to planes final p50 / p95 | GPU, dequant + iDWT p50 / p95 |
| --- | --- | --- | --- | --- |
| 1080p | 1.12 MB | 0.240 / 0.336 ms | 0.406 / 0.505 ms | 0.059 / 0.061 ms |
| 1440p | 1.95 MB | 0.299 / 0.367 ms | 0.514 / 0.579 ms | 0.082 / 0.084 ms |
| 4K | 4.28 MB | 1.751 / 2.838 ms | 2.108 / 3.423 ms | 0.145 / 0.154 ms |

The GPU column is the SDK's own per-pass report: dequantisation plus the inverse wavelet, and
nothing else (not the upload of the records, which the SDK does inside the push and submit). The
"start of push to planes final" column is the honest upper bound of the decode path: the CPU clock
from the first `push_packet` of a frame already complete in memory to the decode fence signalled
(it includes the SDK copying every record and the upload). In the stream the records are pushed as
they arrive, so only the submit and the GPU work remain after the last packet; the bound is for a
frame that arrives whole. The CPU side is the cost, mostly the SDK copying records in
`push_packet`, and scales with the bytes.

### CPU against GPU decode in the stream (ABBA)

The same release build on this PC (RX 7900 XT, client and host on one PC through loopback,
PyroWave 4:2:0, the client window out of the host's capture, `scripts/pyrowave-loopback.ps1
-DecoderA Cpu -DecoderB Gpu`, 60 s per leg, order ABBA per bitrate, the two legs of a decoder
pooled). `receive_to_present_ms` is p50 / p95 / p99; "shown" is the median shown fps; "dropped"
is frames dropped over the decoder queue, summed over the two legs.

| Stream | Mbps | Decoder | receive_to_present p50 / p95 / p99 | decode_to_present p50 / p95 | shown | dropped | decoder errors |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1080p60 | 133 | CPU | 20.53 / 26.85 / 38.29 | 10.85 / 17.04 | 60.19 | 0 | 0 |
| 1080p60 | 133 | GPU | 8.80 / 13.66 / 14.29 | 7.71 / 12.53 | 60.22 | 0 | 0 |
| 1080p60 | 300 | CPU | 25.01 / 46.84 / 62.84 | 10.54 / 27.61 | 60.20 | 0 | 0 |
| 1080p60 | 300 | GPU | 10.59 / 15.68 / 17.63 | 8.00 / 13.05 | 60.20 | 0 | 0 |
| 1440p60 | 149 | CPU | 29.11 / 36.12 / 40.17 | 14.33 / 21.62 | 60.20 | 0 | 0 |
| 1440p60 | 149 | GPU | 9.10 / 14.01 / 14.94 | 7.83 / 12.73 | 60.22 | 0 | 0 |
| 1440p60 | 300 | CPU | 189.79 / 269.26 / 292.05 | 40.84 / 55.98 | 45.44 | 108 | 0 |
| 1440p60 | 300 | GPU | 11.46 / 18.44 / 20.98 | 8.71 / 15.55 | 60.19 | 0 | 0 |
| 4K60 | 167 | CPU | 197.14 / 278.65 / 294.44 | 54.74 / 62.14 | 32.60 | 201 | 0 |
| 4K60 | 167 | GPU | 9.57 / 16.64 / 19.01 | 8.11 / 14.80 | 60.13 | 0 | 0 |
| 4K60 | 300 | CPU | 206.88 / 297.66 / 401.23 | 56.70 / 61.89 | 29.49 | 221 | 0 |
| 4K60 | 300 | GPU | 10.89 / 15.77 / 16.93 | 8.18 / 12.99 | 60.20 | 0 | 0 |

Every leg passes the keep rule (p50 and p95 of `receive_to_present_ms` down, shown fps not lower,
no new decoder errors). The CPU decoder does not keep up at 1440p60 with 300 Mbps or at 4K60 on this CPU (at 4K its
clear, entropy and pack stages take about 5, 6 to 10 and 19 ms a frame), which is where the large
differences come from; the GPU decoder's per-frame cost is 0.2 ms to submit plus 0.2 to 0.3 ms on the GPU, and
the rest of `decode_to_present_ms` is the wait for the present.

On the laptop (Ryzen 7 8845HS, Radeon 780M, 2880x1800 at 120 Hz, over Wi-Fi to this PC's isolated
test host on port 48223; the host's capture is the scripted content on this PC), the same build
with the SDK DLL beside it: the probe passes and the SDK decodes on the 780M. 1080p60 at 133 Mbps,
ABBA, 60 s per leg, the host sent 137.6 to 137.9 Mbps:

| Decoder | receive_to_present p50 / p95 / p99 | decode_to_present p50 / p95 | shown | dropped | decoder errors |
| --- | --- | --- | --- | --- | --- |
| CPU | 23.64 / 29.74 / 31.48 | 9.84 / 16.13 | 60.17 | 0 | 0 |
| GPU | 15.96 / 21.65 / 23.61 | 8.39 / 14.19 | 60.20 | 0 | 0 |

The 7.5 ms p95 of `receive_to_input_ms` on both is the Wi-Fi hop, so the whole difference sits in
decode and presentation.

### The Vulkan to D3D11 handoff

The time from the SDK's work finishing (timeline value signalled) until D3D11 can sample the planes.
There is no copy on the D3D11 side. What D3D11 pays is the fence wait, measured as a queued
`Wait(fence, value)` plus an event query, minus the same event query on an idle queue:

| Size | Wait adds p50 / p95 | Start of push to D3D11-done, no CPU wait p50 / p95 |
| --- | --- | --- |
| 1080p | 0.035 / 0.070 ms | 0.414 / 0.525 ms |
| 1440p | 0.037 / 0.065 ms | 0.524 / 0.584 ms |
| 4K | 0.033 / 0.071 ms | 1.984 / 4.659 ms |

The handoff is about 0.035 ms at the median and about 0.07 ms at p95 at all three sizes, so below
the 0.2 ms line; the 4K total is the push cost of a 4 MB frame (its p95 is that cost's spread), not
the handoff. A D3D12 presenter would not remove anything measurable here.

## Accuracy and handles

- The four fixtures (`pyrowave-{420,444}-{8,10}`) decode on the GPU to within 1 code value of the
  `.yuv` once rounded to a code (unrounded max 1.000, 1.033, 1.000, 1.154; the 10-bit planes differ
  by the half code of the SDK's float arithmetic) and within 0.89 of the CPU decoder (0.674, 0.826,
  0.625, 0.888).
- Lossy frames (lost block ranges, whole frame or records pushed early) match the CPU decoder
  within 0.68 code values.
- Ten decoder lives (create, decode, drop) leave no handles with a shared device that flushes, none
  with each life's own device, and none when created and dropped without decoding (the test bound
  is 10 in all; the earlier run saw 1).
- The reuse fence holds: a decode into a plane set with `after` not yet signalled does not finish
  until the signal, and a ring of three plane sets decoding 24 frames of two different pictures
  back to back with no CPU wait, each read by a D3D11 pass behind the decode fence, gives every frame
  its own fixture's picture (worst 0.674 code values from the CPU decoder, the other picture more
  than 2 away).
- Records pushed early give the same planes as the whole frame, bit for bit, in every lossy case.
- The probe takes 137 ms the first time (one DLL load, one Vulkan device) and 0.006 ms after; the
  10-bit R16 probe succeeds on the RX 7900 XT.

## Tests

`cargo test -p nightjar-desktop` runs the layout, ABI, probe-string, probe-cache, fence-deadline and
`Send`/`!Sync` tests. The GPU tests are
`#[ignore]`d (they need the DLL from `build-pyrowave-sdk.ps1`, a GPU, and no other GPU work):
`cargo test -p nightjar-desktop --release --bin nightjar -- --ignored --nocapture --test-threads=1
pyrowave_sdk::tests`.

## In the stream (brief G2)

- **Decoder.** `Pyrowave::{Cpu, Gpu}` in `stream/pyrowave.rs`. The GPU one feeds each record into
  the SDK as it arrives (early decode), handles partial frames the same way as the CPU one, and
  CPU-waits (2 s) after `decode` so `ready_us` is honest. A frame that does not decode is a frame
  failure (counted, the next frame decodes); no free plane set drops or evicts a picture; a lost
  device or a fence that never signals (`Failure::Device`) ends the stream with the reason, not
  retryable.
- **Picture.** `Picture::GpuPlanes { lease, fence_value }`. The plane sets (queue_limit + 2) are a
  FIFO pool; a `Lease` frees its set when the picture is shown or dropped. The presenter checks
  `ready_within` (500 ms), queues `Wait`, runs `PlanesPass` (the `planes` entry of `pyrowave.hlsl`:
  R8 or R16 UNORM planes in t1..t3 through the same `linearRgb` as the CPU path, into the SDL
  RGBA64F target), signals the reuse fence and flushes.
- **Setting.** `pyrowave_decoder` in the settings file and the settings screen, with the probe
  result cached in the background for the settings screen. The overlay and the session summary
  name the decoder ("PyroWave (GPU decode, SDK)" or "PyroWave (Rust, GPU wavelets)").
- **Known open issue: the GPU picture is darker.** On real 8-bit frames the GPU picture is 0.29
  of a code darker than the CPU decoder's on average (signed mean -1.12e-3 in encoded RGB, the same
  on every one of the 24 dumped 1080p frames; the fixtures show none). The cause is suspected in
  the SDK's 8-bit plane store (R8) and has not been isolated. It is not fixed: the real-frames
  test bounds the signed mean at 0.4 code (`REAL_MEAN_LIMIT`) so a larger offset or gain error
  still fails, and the fixtures keep 0.25 code.
- **Accuracy.** The GPU picture against the CPU picture, encoded RGB, SDR 4:2:0 and 4:4:4
  fixtures, 24 real 1080p frames from `NIGHTJAR_PYROWAVE_DUMP`: 10-bit planes max 0.0038 (0.98
  codes), rms 0.0007, inside the brief's bound (max under 1/255, rms under 0.001). 8-bit planes
  max 0.0082 (2.1 codes), rms 0.0020 on the fixtures and 0.0017 on real frames: the SDK stores 8-bit
  planes as R8 (rounding of about 0.29 code rms per plane), and that plus the 1.1-code decoder
  agreement measured in G1 goes through the YCbCr matrix; the bound for 8-bit is therefore 2.5/255
  and 0.0025.
- **Tests.** `cargo test -p nightjar-desktop` has the pipeline policy tests (device loss ends the
  stream, frame failure and ring-full policy, `gpu` mode without a device gives no CPU decoder) and
  the mode tests (`gpu` with a failing probe offers no PyroWave and shows the reason, `cpu` never
  loads the DLL). The `#[ignore]` GPU tests in `stream/pyrowave/gpu_tests.rs` (run serially:
  `cargo test -p nightjar-desktop gpu_tests -- --ignored --test-threads=1`, with
  `NIGHTJAR_PYROWAVE_FRAMES` for the real-frame one) cover picture equality, early chunks with and
  without lost packets, frame failures then recovery, the ring, eviction and reuse, and a stalled
  fence.
- **Review fixes.** With HostTimes pacing the pacer's queue is shared with the decoder thread
  (`Shared.pacer_backlog`), so a full plane ring evicts the oldest queued picture instead of
  starving the decoder; a sequence header whose size differs from the stream's is a frame failure;
  `PlanesPass` gives the plane set back when the reuse `Signal` fails and drops the old decoder's
  sets when it is re-targeted; the settings screen probes only for PyroWave (on the renderer's
  adapter and HDR state); the CPU-mode early/fallback counters are kept on failed frames. The
  accuracy test also bounds the signed mean (0.25/255) and pins the 10-bit UNORM16 scale (code
  over 1023, not code shifted by six).

### Beam race: slice decodes (B10 step 4)

With `beam_race` on and the host sending row order, the final frame is preceded by up to three
slice decodes of the rows that have arrived. Loopback, 60 s, 1080p60, 399 Mbps: 4 slices on every
one of 3589 frames, first slice ready 1.65 ms (p50) after the first packet against 6.48 ms on the
wire, last 7.20 ms. The final decode is byte-identical to a decode with slicing off on all three
planes, and each slice matches that reference on its rows (ignored GPU test, 24 frames). See
[beam-racing.md](beam-racing.md).

## The wait between decoded and shown (HostTimes pacing)

Measured with the 1080p/4K 300 Mbps PyroWave loopback on the dev PC (display 5120x1440 at 240 Hz,
tearing/VRR present, so `PacingMode::HostTimes`), GPU decode, 60 s ABBA legs, first 5 s dropped, two
legs per variant pooled. `scripts/timing_breakdown.py <ABBA folder>` prints the split from the timing
CSV (`due_ns`, `present_begin_ns`, the swap chain's frame statistics; see `timing_csv.rs`).

| 1080p 300 | HostTimes (A) | Immediate (B, `present_at_host_times = false`) |
|---|---|---|
| hold (decoded to due) p50 / p95 / p99 | 7.21 / 12.22 / 12.94 | 0 |
| due to present call (ms) p50 / p95 | 0.04 / 0.51 | 0.04 / 0.07 |
| present call (draw and `SDL_RenderPresent`) p50 / p95 | 0.58 / 0.82 | 0.72 / 0.99 |
| decode_to_present p50 / p95 / p99 | 7.92 / 12.93 / 13.70 | 0.76 / 1.05 / 1.18 |
| receive_to_present p50 / p95 / p99 | 10.36 / 15.35 / 16.08 | 3.21 / 3.97 / 4.22 |
| shown fps | 60.10 | 60.10 |
| present interval p50 / p99; abs(interval - 16.67) p99 | 16.61 / 25.70; 9.24 | 16.66 / 18.34; 1.96 |

| 4K 300 | HostTimes (A) | Immediate (B) |
|---|---|---|
| hold p50 / p95 / p99 | 7.47 / 13.11 / 15.22 | 0 |
| receive_to_present p50 / p95 / p99 | 10.92 / 16.72 / 19.25 | 3.46 / 4.44 / 4.83 |
| decode_to_present p50 / p95 | 8.27 / 14.07 | 0.91 / 1.38 |
| shown fps | 60.05 | 59.76 |
| present interval p99; abs(interval - 16.67) p99 | 28.17; 11.51 | 21.80; 6.51 |

- The hold is the whole wait: the pacer's hold is 7.2 to 7.5 ms at p50, the presenter starts
  0.04 ms after the due time and the draw and present call take 0.6 to 0.9 ms. Nothing in the
  presenter adds to it.
- Flip time cannot be read here: with a tearing present the swap chain's frame statistics
  (`GetFrameStatistics`) report a vblank that has no relation to the present (the `call->flip`
  rows of the script, 20 to 500 ms). A tearing present is scanned out at the next scan line, at
  most 1/240 s (4.2 ms) after the call. Refresh rate 240 Hz, tearing true (the client log line
  "renderer direct3d11, display 5120x1440 at 240.00 Hz, tearing true").
- Why the clock wants about 7 ms: `PresentClock` waits for the 97th percentile of lateness
  (ready minus the host's frame stamp plus the clock offset) plus 0.5 ms. On this stream lateness
  is p50 10.1, p97 16.9 ms (spread 6.8 ms). The spread is almost all host side: capture to
  acquired 0.1 ms, acquired to submitted p50 5.5 / p97 12.1 ms (the host keeps the newest of the
  240 Hz captures until its 60 fps encode tick, so a stamp is up to 16.7 ms old when encoding
  starts), encode 1.0, send 1.0, network 0.1, receive to decoded 2.4 (spread 0.7). So the p97 rule
  is paying for the host's sample-and-hold age, which the client sees only as lateness. HostTimes
  reproduces the stamps' cadence (the intervals follow the host's stamps, which are 4.2 ms
  quantised on a 240 Hz display), which is why its present intervals are more uneven than
  Immediate's here: Immediate shows frames in the order and cadence they arrive.
- Tighter rule tried: percentile 90 instead of 97 (hold p50 8.6 to 6.0 ms at 1080p and 7.7 to 6.2
  ms at 4K; receive_to_present p50 12.3 to 9.4 and 11.2 to 9.7, p95 19.3 to 14.9 and 17.6 to 15.4;
  A legs are noisy, one 1080p A leg sat at 14.5). Interval jitter p99 (abs deviation) 13.1 to 12.7
  at 1080p and 12.5 to 16.6 at 4K, shown fps 58.7 to 59.0 and 59.6 to 58.4: more late frames at
  4K. Not shipped: it fails the "jitter no worse than 1 ms more" bar at 4K and the gain is within
  the leg-to-leg spread of 1 to 2 ms.
- **Default changed: `present_at_host_times` is now off**, so a VRR/tearing display presents
  immediately (`PacingMode::Immediate`; the setting "Show frames at the PC's frame times" is the
  opt-in for `HostTimes`). On this display Immediate beats HostTimes on latency (receive_to_present
  p50 3.21 against 10.36 ms at 1080p, 3.46 against 10.92 at 4K) and on smoothness (present-interval
  p99 deviation 1.96 against 9.24 ms at 1080p; 6.51 against 11.51 at 4K). Displays without tearing
  support still wait for vsync, as before. Immediate follows the arrival cadence instead of the
  PC's frame times; a source with a small host-side
  spread (a game presenting right before the capture) gets a much smaller hold than this
  synthetic desktop animation. Cutting the host's acquired-to-submitted age (submit the capture
  closest to the encode tick, or stamp the frame with its submit time) would shrink the spread at
  the source and is a host change.

## Dropping the CPU wait on the decode fence: measured, not shipped

The decoder thread's `GpuDecoder::wait` (2 s timeout) after `decode` was removed in a trial build
(the presenter's GPU `Wait` and the 500 ms `ready_within` check already cover a decode that does
not finish). GPU decode, ABBA, old (A) against trial (B), two legs each, pooled:

| 300 Mbps | receive_to_present p50 A / B | p95 A / B | p99 A / B | decode_to_present p50 A / B |
|---|---|---|---|---|
| 1080p | 11.02 / 11.30 | 16.51 / 17.89 | 18.83 / 19.51 | 8.14 / 8.53 |
| 4K | 11.82 / 11.71 | 18.65 / 18.83 | 20.35 / 20.06 | 8.83 / 9.07 |

p95 was worse in both, and the leg to leg spread (10.9 to 13.3 ms between legs of the same
variant) is larger than any difference, so there is no gain to keep: reverted. The wait stays.

## Skipping to the newest frame (CPU decode): measured, not shipped

PyroWave frames are intra-only, so a decoder that has a backlog can drop the older frames and decode the newest. The candidate did that in the frame queue (a frame with a newer one behind it is skipped, and early chunks of a frame that is certain to be skipped are dropped), with the skipped frame still passed through `Records::push_frame(data, |_| true)` so the recovery cache stays where a decode would have left it. It lives on the local branch `pwd2-skip-candidate` (two commits, tests included); it is not on `main`.

Measured on this PC over loopback: Rubylight host, 1080p60 PyroWave 4:2:0, 60 s legs, release builds, parent (A) against candidate (B), with the relaunch fix on both.

| Run | Order | Leg | `receive_to_input_ms` p95 / p99 | `input_to_output_ms` p50 | shown fps | Frames skipped |
|---|---|---|---|---|---|---|
| 133 Mbps | ABBA | A | 0.68 / 0.82, 0.64 / 0.78 | 5.71, 5.93 | | |
| 133 Mbps | ABBA | B | 0.68 / 0.85, 0.65 / 0.81 | 6.00, 6.04 | | 0, 0 |
| 300 Mbps, run 1 | ABBA | A | 1.73 / 8.50, 4.69 / 25.27 | 8.65, 8.71 | 58.8, 59.5 | |
| 300 Mbps, run 1 | ABBA | B | 1.68 / 7.21, 0.79 / 2.35 | 8.02, 7.29 | 51.8, 57.3 | 6, 0 |
| 300 Mbps, run 2 | BAAB | A | 0.65 / 0.81, 0.64 / 0.76 | 5.87, 5.77 | 55.7, 57.8 | |
| 300 Mbps, run 2 | BAAB | B | 0.65 / 0.83, 0.63 / 0.76 | 6.22, 5.93 | 56.7, 57.0 | 0, 1 |

What it shows:

- The host sends about 88 Mbps at a 300 Mbps target (PyroWave sends what the content needs), so the decoder keeps up in most legs. `queue_wait_ms` p95 is 0.04 to 0.05 ms in 8 of the 12 legs (all four at 133 Mbps and all four of the second 300 Mbps run), and B skipped at most one frame there. The backlog only appeared in the first 300 Mbps run (queue wait p99 up to 24.7 ms in one A leg) and did not come back in the second.
- When a backlog did form, B looked better (p99 2.35 and 7.21 ms against 8.50 and 25.27 ms), but B's best leg skipped no frame at all, so the gap is leg to leg variation and not the skip. The two A legs differ from each other by the same margin.
- One B leg in that run showed 51.8 fps against 58.8 and 59.5 for A, with the host sending fewer frames (79 Mbps against 87 to 89); the repeat run did not show it. Cause not found.
- Without a backlog B is slightly slower: `input_to_output_ms` p50 is 0.2 to 0.3 ms higher in the 133 Mbps run and in the second 300 Mbps run.

Verdict: the keep rule fails (the p99 gain is not separable from noise, one B leg lost fps, and `input_to_output_ms` p50 is worse when nothing is queued), so the change is reverted. The GPU decoder removes the CPU decode time that builds the backlog, and skipping stays an option there if a slow decode ever queues frames.

Not measured: 1440p and 4K, the early-decode leg, and a forced backlog (for example a throttled decoder) that would exercise the skip on purpose. The Nightjar timing CSV leaves its fps columns blank, so shown fps above is the rendered frames over their span.
