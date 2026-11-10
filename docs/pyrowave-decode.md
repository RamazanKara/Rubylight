# PyroWave GPU decoding in the desktop client

[Documentation](index.md)

Status: shipped. The `pyrowave_decoder` setting picks `Cpu` (the Rust
decoder in `stream/pyrowave.rs`) or `Gpu` (the SDK, this document). Neither falls back to the other:
with `Gpu` and a failing probe, PyroWave is not offered and the settings screen says why (the probe's
error text, "Switch the PyroWave decoder to CPU ..."); with `Cpu` the DLL is never loaded.
The default is `Gpu` on Windows (every leg of the ABBA below keeps, on this PC and on the 780M
laptop, and on the NUC under Linux, [below](#linux-brief-g3)). A settings file that says `Cpu` keeps it.

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
- **Fixed: the CPU path's picture was 0.28 code too bright, not the GPU's too dark.** On real 8-bit
  frames the GPU picture used to measure 0.29 of a code darker than the CPU path's (signed mean
  -1.12e-3 in encoded RGB, the same on all 24 dumped 1080p frames). Isolated against the CPU decoder's
  own float planes run through the same YCbCr matrix: the SDK's R8 planes are right (mean +0.01 code
  from the floats, nearest-rounded) and the GPU picture is within -0.015 code; the CPU path was +0.28
  code, +0.34 in the black parts, -0.1 in white. Its wavelet passes keep samples in half-float
  targets (ten stores per frame) and Direct3D 11 on the RX 7900 XT cut the mantissa instead of
  rounding it. Simulating ten toward-zero stores on the f32 reconstruction reproduces it; round to
  nearest does not. `storedHalf` in `pyrowave.hlsl` (Direct3D only; the Vulkan module is unchanged)
  rounds to the 11 significant bits of a half before the store, so the cut loses nothing. Now the CPU
  path is -0.006 and the GPU picture -0.014 code from the floats, and the two pictures differ by
  -3.8e-5 (0.01 code) on real frames, max 1.5 codes, rms 0.0007 to 0.001. Regression tests:
  `both_pictures_match_the_float_planes_on_average_on_dumped_real_frames` (0.1 code),
  `REAL_MEAN_LIMIT` (0.1 code) and `MEAN_LIMIT` (0.05 code) in `stream/pyrowave/gpu_tests.rs`. Not
  measured: Intel or NVIDIA GPUs (the cut may be AMD's; rounding twice is exact), and Vulkan, which
  rounds to nearest by specification.
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

## Linux (brief G3)

The same SDK, loaded as `libpyrowave-shared.so` from beside the executable
(`stream/pyrowave_sdk/unix.rs`). `scripts/build-pyrowave-sdk.sh` builds the pinned commit and
patches into `~/nj-pyrowave-sdk` (or `-o <folder>`) with `build-info.txt` and `licenses/`. There is
no shared-texture handoff to the presenter on Linux, so the route differs from Windows after the
decode:

- **Decode.** The SDK creates its own Vulkan device (`create_device_by_compat` with no LUID, so the
  Vulkan driver's default adapter, which on a machine with several GPUs is not necessarily the
  renderer's) and decodes with `decoder_decode_cpu_buffer_async` into a ring of plane-buffer sets
  (three CPU buffers each, R8 or R16 planes, tightly packed rows). The buffers are plain memory, so
  there is no fence to share: `Picture::GpuPlanes { lease }` holds the set and dropping the lease
  (shown, or dropped unshown) gives it back. The setting, the probe, early decode and the failure
  policy are the Windows ones.
- **Present.** `stream/planes_gpu.rs` (`PlanesPass`) copies the three planes into one SDL GPU
  transfer buffer, uploads them to three persistent R8/R16 UNORM textures on the renderer's own
  device (its own copy pass, submitted before the render pass) and draws the `planes` entry of
  `pyrowave.hlsl` through `SDL_GPURenderState` into the RGBA64F picture texture: the same shader
  and the same `linearRgb` as the CPU path. The upload and the draw are the presenter's `upload` and
  `passes` stages. The renderer must be SDL's "gpu" one (Vulkan, SPIR-V); anything else is the same
  loud "PyroWave GPU decoding needs ..." refusal as on Windows.
- **Shader module.** `pyrowave-planes.spv` is glslang's output (dxc is not used here):
  `glslangValidator -V -D -e planes -S frag --auto-sampled-textures --target-env vulkan1.0
  protocol/desktop/src/pyrowave.hlsl -o protocol/desktop/src/pyrowave-planes.spv`. `-V` defines
  `VULKAN`, which selects the sampler slots t0 to t2 in `space2` that SDL's fragment shaders use.
  Check the result with `spirv-val`. glslang 15.1.0 (Ubuntu's `glslang-tools`) reproduces the
  committed module byte for byte. `pyrowave-planes.spv.sha256` beside it records the SHA-256 of
  `pyrowave.hlsl` (with LF line endings) and of the module, and
  `pyrowave_gpu::tests::the_planes_module_was_built_from_the_shader_as_it_is` fails when either
  file changes without the other: after editing the shader, regenerate the module and update
  both hashes (`sha256sum`). The committed `pyrowave.spv` is a different module and is not
  regenerated.
- **Probe and the loud failure.** Without the library the client refuses the stream, it does not
  switch to the CPU decoder: `autostream failed: PyroWave GPU decoding unavailable:
  libpyrowave-shared.so not found next to the executable (...). Switch the PyroWave decoder to CPU
  in the settings, or pick another codec.` A failing Vulkan device gets the same treatment, with the
  SDK's error. Probing twice loads the SDK once.
- **Packaging.** `protocol/desktop/package-linux.sh -f <FFmpeg prefix> -s <SDK folder>` builds the release
  binary against the decode-only FFmpeg (shipped in `lib/`, see `protocol/desktop/README.md`) and
  packs `nightjar-desktop-<version>-linux-<arch>.tar.gz` with the library beside it, its build-info
  as `pyrowave-sdk-build-info.txt`, `licenses-pyrowave/` and `system-libraries.txt` (what `ldd`
  finds). Like the Windows script it refuses a library that is not the pinned one, or whose SHA-256
  is not the recorded one, before the long build.
- **Default.** `Gpu`, as on Windows. `package-linux.sh -s` puts the library in the package. A
  source build without it, or a box that fails the probe, behaves as a Windows one without the DLL:
  PyroWave is not offered in the settings, with the reason and the hint to switch the decoder to
  CPU, and asking for it anyway (`NIGHTJAR_*` autostream) is the exit-2 refusal above. No other
  codec is affected, so a GPU default does not make a library-less build unusable; only PyroWave
  needs the setting changed there.
- **Deadline.** `pyrowave_decoder_decode_cpu_buffer_complete` waits on its fence with no timeout,
  so submit and complete run together as one job on a wait thread (`pyrowave_sdk/watchdog.rs`; the
  SDK registers its thread index and logger in the submit call, so that thread is set up by the
  call itself, and one hand-over per frame covers both) and `decode` gives up after the same
  `GPU_DECODE_TIMEOUT` (2 s) as Windows, with `Failure::Device("PyroWave GPU decode failed: the
  PyroWave decode did not finish within 2s: the Vulkan device may be hung or lost")`. A stalled
  decoder is never called, destroyed (that would idle the hung device and block) or reused again,
  and the plane set of that decode is leaked, because the stuck call may still copy into it. The
  deadline logic is unit-tested everywhere (`watchdog::tests`); an ignored GPU test forces a miss
  with a zero deadline.

### Linux measurements

Client: the NUC (Core i5-8259U, Iris Plus 655, Mesa 25.0.7, Debian 13). It also runs other
containers, so its load is not quiet. The client ran in a container with `/dev/dri`, SDL's
`offscreen` video driver (1024x768) and the "gpu" renderer, so nothing is scanned out and
`receive_to_present_ms` ends at the offscreen present. Host: this PC (RX 7900 XT), isolated profile
on port 49123, `virtual_display_layout = extended`, 1080p60 over 1 GbE, 60 s legs in ABBA order
(`scripts/pyrowave-linux.ps1 -Bitrates 133000,300000`; the host sent about 137.8 and 312.5 Mbps).
Raw results are in `C:\src\nj-pwd-out\linux-abba`.

Equality first. The SDK on the NUC against the reference `.yuv` fixtures: max error 1.0 to 1.15
code values, 0.62 to 0.89 against the CPU decoder. The presenter's pictures against the CPU
pictures (`pyrowave/gpu_tests_unix.rs`, encoded RGB): max 0.8 to 2.1 codes, inside the Windows bounds
(2.5/255 for 8-bit, 1/255 for 10-bit). The SDK alone, push to planes in memory: 1080p p50 4.9 ms
(p95 5.3, GPU 2.3), 1440p p50 8.3, 4K p50 19.0.

| Stream | Mbps | Decoder | receive_to_present p50 / p95 / p99 (ms) | decode_to_present p50 / p95 (ms) | shown fps | dropped | decoder errors |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1080p60 | 133 | CPU | 23.73 / 231.16 / 278.95 | 6.13 / 17.92 | 58.91 | 16 | 0 |
| 1080p60 | 133 | GPU | 13.45 / 25.23 / 30.07 | 1.39 / 2.87 | 60.09 | 0 | 0 |
| 1080p60 | 300 | CPU | 148.04 / 275.06 / 291.52 | 7.27 / 11.19 | 49.28 | 70 | 0 |
| 1080p60 | 300 | GPU | 19.37 / 30.23 / 44.10 | 1.47 / 2.98 | 60.00 | 0 | 0 |

Both bitrates keep G2's rule (`timing_summary.py --keep`: p50 and p95 of `receive_to_present_ms`
not worse, no fps loss, no new decoder errors). On this CPU the Rust decoder's entropy stage takes
about 6 ms a frame at 133 Mbps and 13.7 to 15.2 ms at 300 Mbps, so the queue fills at 300 Mbps and
drops frames. One GPU leg at 300 Mbps was noisier (p50 22.9 ms, the SDK's `gpu` stage 11.4 ms
against 4.6 to 5 ms elsewhere) and is still better than both CPU legs. Not measured: a Linux
machine with a discrete GPU, a real display (the offscreen driver has no scan-out or vsync),
Wayland or X11 presentation.

Tests on Linux: `cargo test -p nightjar-desktop -p nightjar-core` runs G2's mode and probe tests.
The ignored ones need the library and a Vulkan device, one at a time:
`NIGHTJAR_PYROWAVE_SDK=<folder> cargo test -p nightjar-desktop --release -- --ignored
--test-threads=1 pyrowave_sdk gpu_tests_unix`. `pyrowave_sdk::tests` is the G1 set (fixtures against
the reference and the CPU decoder, lossy frames, a ring of buffers, probing twice, decode timing);
`gpu_tests_unix` runs the fixtures through the presenter on an offscreen "gpu" renderer against the
CPU pictures, plus a sequence longer than the ring. Four older ignored tests (wavelet tests needing
`XDG_RUNTIME_DIR`, one needing `NIGHTJAR_PYROWAVE_FRAMES`) fail on the headless NUC for their
own environment reasons. `scripts/pyrowave-linux-leg.sh` runs one client leg on the NUC.

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

## Decoding on an async compute queue: a Video setting, off by default

Nonary's moonlight-qt fork added async compute for PyroWave (`pyrowave_device_set_queue_type` with the compute
family, commit 5bb5902, "no measurable render-completion difference" on the Steam Deck) and reverted
it two days later (e225f88): async decodes delayed Wayland presentation of frames that were already
GPU-ready. Their client now passes the compute family to the SDK but decodes on the graphics queue.

The desktop client has it as the setting **Decode PyroWave on a compute queue** in the Video
section, under PyroWave decoder (`pyrowave_async_compute` in the settings file, off by default;
`stream/pyrowave_sdk/queue.rs`). On a GPU without a separate compute queue it changes nothing.
Off, nothing changes: the SDK decodes on the graphics queue as before, and on Linux the renderer's
device is made exactly as before. On, the decodes go to a separate compute queue where the GPU has
one, so a decode can overlap the previous frame's drawing and present:

- **The queue.** Granite's rule, which the SDK's own device follows (`queue::compute_queue`, unit
  tested): the first queue family with compute but not graphics that has a queue left (AMD's and
  NVIDIA's compute family), else a second queue of a compute family (a second queue beside the
  graphics one). A GPU with neither keeps the graphics queue.
- **Windows and the Linux readback** (the SDK's own Vulkan device): Granite makes that queue
  itself, and `pyrowave_device_set_queue_type(VK_QUEUE_COMPUTE_BIT)` moves the decodes onto it before
  the decoder is created. The planes are acquired from `QUEUE_FAMILY_IGNORED` and released to
  `QUEUE_FAMILY_EXTERNAL` by the decode itself, and the decode fence (Windows: the shared
  `ID3D11Fence` D3D11 waits on; Linux: the SDK's fence before the readback) orders the presenter after
  it, as on the graphics queue.
- **Linux, the renderer's shared device** (`zero_copy.rs`): the patched SDL makes one more queue by the
  same rule when the renderer's device is created with the setting on
  (`SDL_PROP_GPU_DEVICE_CREATE_VULKAN_COMPUTE_QUEUE_BOOLEAN`, `vulkan-interop.patch`), and the SDK is
  handed it beside SDL's queue, under SDL's queue lock like every other submission. The plane images
  are then shared by both families (`VK_SHARING_MODE_CONCURRENT`) when the queue is in another family.
  The decode waits on the GPU for the reuse timeline SDL's queue signals after the copy that last read
  its plane set, and signals the decode timeline, which the decoder waits for before the presenter's
  copy is recorded. The renderer's device is made when Nightjar starts (or when it switches renderer
  for a stream), so turning the setting on takes effect there with the next device; until then the log
  says the device has no compute queue.
- **The log.** Every stream start says where PyroWave decodes and why: `PyroWave decodes on a
  separate compute queue: family 1, queue 0`, or `PyroWave decodes on the graphics queue:` followed
  by the reason (the setting is off, the GPU has no compute queue apart from the graphics one, the
  renderer's device was made while the setting was off, or the SDK refused the queue).
- **Tests.** The queue choice, the setting's default and persistence, the shared device's queue list
  and the plane images' sharing mode are unit tests. The ignored GPU test
  `gpu_tests_unix::the_compute_queue_decodes_the_pictures_of_the_graphics_queue` decodes the fixtures
  on the compute queue (shared device and readback) against the graphics queue's pictures.

### Measured: no clear gain on the 780M laptop, so off by default

2026-11-09, the laptop (Ryzen 7 8845HS, Radeon 780M, Windows 11, 2880x1800 at 120 Hz, the client
windowed) over Wi-Fi from this PC's isolated test host (port 48463), 1080p60 PyroWave 4:2:0 asked at
133 Mbps (the host sent 89.5 to 94 Mbps), GPU decode in both, the same release build, 60 s legs in
ABBA order, A with the setting off, B on, the two legs of each pooled
(`scripts/pyrowave-laptop.ps1 -DecoderA Gpu -DecoderB Gpu -ExtraB @{ pyrowave_async_compute = $true }`).
The B legs' logs say `PyroWave decodes on a separate compute queue: family 1, queue 0`, the A legs'
the graphics queue. Raw results: `C:\src\nj-pwd-out\laptop-async-compute-1`.

| Variant | receive_to_present p50 / p95 / p99 | per leg p50 / p95 | shown fps (legs) | dropped | SDK gpu stage p50 / p95 |
| --- | --- | --- | --- | --- | --- |
| A, graphics queue | 10.51 / 11.95 / 13.21 | 10.53 / 11.93, 10.49 / 11.99 | 59.48 (60.04, 58.93) | 1, 0 | 0.57 / 0.73, 0.57 / 0.73 |
| B, compute queue | 10.41 / 11.91 / 13.00 | 10.40 / 11.88, 10.42 / 11.91 | 58.19 (57.34, 59.04) | 0, 0 | 0.56 / 0.71, 0.56 / 0.72 |

B is 0.1 ms lower at p50 and 0.04 ms at p95; the p95 difference is inside the leg to leg spread (0.06
ms between A's legs), and the shown fps median is lower on B (one B leg at 57.34), so it fails the keep
rule (`timing_summary.py --keep`: REVERT on shown fps). The SDK's GPU time per frame is the same on
both queues (0.56 to 0.57 ms), as the decode is short beside a 16.7 ms frame. The setting stays off by
default; it stays a setting, in Video next to PyroWave decoder, for comparing on other GPUs.

Not measured: this PC (it served as the host), a Wayland compositor, NVIDIA and Intel with a compute
queue. The NUC under Linux (Iris Plus 655, Mesa) has one queue family with one queue, graphics and
compute, so the setting changes nothing there (its log says the GPU has no compute queue apart from
the graphics one) and it was not run. The upstream caveat (delayed presentation on Wayland) stands.

Android keeps the graphics queue and has no such setting: its renderer creates one graphics and
compute family (decoding on its queue 0 and presenting on queue 1 where the family has two) and
prefers the fragment path on mobile GPUs, which needs the graphics queue, so there is no separate
compute queue to hand the SDK.

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

## Skipping to the newest frame when a backlog forms (desktop, CPU and GPU decode): shipped, measured on the 780M laptop (GPU decode)

A decoder whose decode takes longer than the frame interval never catches up: the 15-frame decode queue fills, every frame waits behind the ones before it, and the full queue is flushed with a key-frame request, after which it fills again. A reported 4K 112 fps session on an integrated GPU showed that (GPU decode about 11 ms a frame against an 8.9 ms interval, a queue overflow about once a second). That configuration (10-bit HDR, Linux) was not reproduced here; the same ratio of decode time to frame interval was, with a larger picture (below).

The decode loop (`session/video.rs` in `client/protocol/client`) now skips for a decoder that reports `intra_only()`:

- A frame is skipped when two or more newer frames are queued behind it (`SKIP_BACKLOG`). Within one second of the last skip (`SKIP_HOLD_US`) one newer frame is enough, so a decoder that is steadily too slow stays at the newest frame instead of oscillating between a backlog of zero and two.
- With nothing queued behind the frame the rule is one integer comparison and nothing changes; a pair of frames that arrive together is not a backlog and both are decoded. That is the case the candidate above made 0.2 to 0.3 ms slower.
- Early chunks of a frame that is certain to be skipped are dropped. A skipped frame goes to `VideoDecoder::skip`, which on desktop passes it through the records cache (`Records::push_frame`, or `finish_frame` when the early chunks already held the whole frame) so recovery of a later lossy frame reads the same cache a decode would have left. A skipped key frame is reported decoded with the next frame that is decoded.
- Desktop reports `intra_only()` for PyroWave on the CPU and on the GPU, except while beam racing is active. Other codecs and Android are unchanged (Android's decoder keeps its own records cache behind JNI and has no skip entry yet).

Skipped frames are counted with the overlay's "Decoder drops" line and in the session-end log line ("skipped for a newer frame").

The unit tests drive a gated decoder that is slower than its stream and check that the queue never holds more than three frames, that the newest frame is always decoded and that no key frame is requested.

Measured on the 780M laptop (Windows 11, 2880x1800 at 120 Hz, Wi-Fi 6E at 160 MHz) from an isolated Rubylight host on the RX 7900 XT PC: PyroWave 4:2:0 8-bit SDR at 120 fps, 1.4 Gbps configured (about 1.24 Gbps sent), GPU decode through the SDK, pacing Immediate, overlay off. The host captured an unchanged 2560x1440 desktop, scaled it to the stream size and resent the still picture at 120 fps, so a frame left every frame period (the host sent 116 to 120 fps in every leg). A is the released 4.12.3 Windows build, B a release build of this change run beside the same DLLs. Percentiles come from the timing CSV (one row per picture shown) after the first 5 s; frame counts are the session-end line's and cover the whole leg; key-frame requests are the host's `idr_requests`, which counts the one that starts a stream. "End to end" is the overlay's figure computed per row: the present call's return minus the frame's host timestamp moved onto the client's clock.

| Stream | Leg | `input_to_output_ms` p50 / p95 | `queue_wait_ms` p50 / p95 | `receive_to_present_ms` p50 / p95 | End to end p50 / p95 | Decoded | Skipped | Queue overflows | Key-frame requests | Shown fps |
|---|---|---|---|---|---|---|---|---|---|---|
| 7168x4032, 45 s, order ABBA | A | 10.60 / 10.96, 11.08 / 11.43 | 57.0 / 120.3, 58.2 / 118.2 | 80.3 / 143.3, 82.8 / 142.4 | 87.9 / 149.6, 89.5 / 150.3 | 4398, 4171 | | 63, 78 | 64, 79 | 22.8, 21.5 |
| 7168x4032, 45 s, order ABBA | B | 10.62 / 11.13, 10.73 / 11.02 | 4.2 / 8.9, 4.2 / 8.8 | 27.1 / 33.4, 27.4 / 33.4 | 33.9 / 40.4, 33.6 / 40.2 | 4252, 4238 | 1164, 1091 | 0, 0 | 1, 1 | 21.9, 21.7 |
| 7680x4320, 30 s | A | 12.35 / 12.54 | 40.6 / 118.8 | 69.5 / 145.7 | 78.5 / 153.2 | 2728 | | 56 | 56 | 22.8 |
| 7680x4320, 30 s | B | 12.11 / 12.43 | 4.0 / 9.4 | 32.0 / 37.2 | 38.5 / 45.5 | 2550 | 1062 | 0 | 1 | 19.8 |
| 6400x3600, 45 s | A | 5.15 / 5.62 | 0.45 / 0.59 | 24.7 / 30.6 | 31.1 / 38.1 | 5409 | | 0 | 1 | 100.8 |
| 6400x3600, 45 s | B | 5.08 / 5.43 | 0.40 / 0.53 | 23.7 / 26.8 | 29.4 / 32.6 | 5236 | 4 | 0 | 1 | 112.6 |
| 3840x2160, 30 s | A | 2.55 / 2.71 | 0.15 / 0.24 | 9.8 / 11.6 | 13.7 / 18.2 | 3652 | | 0 | 1 | 120.1 |
| 3840x2160, 30 s | B | 2.64 / 2.77 | 0.14 / 0.18 | 9.7 / 11.2 | 13.1 / 17.1 | 3508 | 0 | 0 | 1 | 116.8 |

What it shows:

- 4K at 120 fps does not make this laptop's decoder slower than the stream: a frame decodes in about 2.6 ms and the stream shows 117 to 120 fps at 13 to 14 ms end to end with either build. The reported 11 ms at 4K is therefore not what a 780M needs for the decode as such; which of 10-bit HDR, Linux and its compositor makes the difference is not measured. A decode slower than the 8.33 ms interval needed a picture of 7168x4032 here.
- With the decoder slower than the stream (7168x4032 and 8K), A waits in the queue for 41 to 58 ms at the median and about 120 ms at p95, overflows the queue 1.4 to 1.9 times a second and asks for a key frame each time. B waits 4 ms at the median and 9 ms at p95 (the longest wait was 11 ms, one decode), never overflows and asks for no key frame after the first. End to end falls from 88 to 90 ms to 34 ms at the median and from 150 ms to 40 ms at p95 (7168x4032, both orders).
- B skips 20 to 22 % of the frames at 7168x4032, which is what a 10.7 ms decode against an 8.33 ms interval leaves no time for (22 %), so two frames and one second do not skip more than the decoder's rate requires. Decode time is the same in A and B.
- With the decoder faster than the stream the rule stays out of the way: 0 skips in the 4K leg and 4 of 5240 frames at 6400x3600, with the queue wait unchanged.
- Shown fps at 7168x4032 and 8K is about 22 in both builds: the presenter replaced about three of four decoded pictures before showing them. That is a limit of presenting pictures this large on this laptop, not of the decode queue (the reported 4K session replaced 0 to 2 pictures), and it is not explained here. The shown fps difference at 6400x3600 is one leg each.
- The host logged transient UDP send failures in three A legs (46, 46 and 310 packets) and in no B leg; the client's health line reported 0.0 % worst loss in every leg.

Not measured: 10-bit HDR, Linux, the CPU decoder's skip path on hardware, and the time from the present call to the screen.

## Beam racing: measured, opt-in (helps the median at low pace, not p95, worse `receive_to_present` at high pace)

The idea: with the host sending each row of blocks with the data it depends on (`pyrowaveRowOrder`),
decode the picture in slices while it is still on the wire and present each slice before the display's
beam reaches its rows, so a high PyroWave bitrate stops costing the time a whole frame takes to arrive.
It was built in three pieces and measured against presenting whole pictures. It missed its default-on
gate on loopback, so it ships as an opt-in: the `beam_race` setting (off by default) or
`NIGHTJAR_BEAM_RACE=1`. Off, it costs nothing: no row order is asked of the host, no scan line probe
runs, no slice is decoded or handed over, frames decode and present as without it
([beam-racing.md](beam-racing.md)). On, a stream that cannot race says why in the log and on the stats
overlay instead of quietly presenting whole pictures.

What the pieces showed, all on this PC (RX 7900 XT) with the Nightjar desktop client in borderless
fullscreen on the 5120x1440 240 Hz VRR display, loopback, 1080p60 PyroWave at 399 Mbps:

- **Slice decodes** (brief 1) worked: with the row order, the first of four slices was decoded 1.65 ms
  (p50) after the frame's first packet against 6.5 ms for the frame on the wire, the final decode
  byte-identical to a whole decode and every slice exact on its rows. The early-decode leg's "0
  frames decoded ahead" was the client not parsing a `3.0.0-beta` host version (fixed, stays).
- **The probe** (brief 2): a tearing present on this display restarts the scan after a median 0.91 ms,
  the scan takes 4.17 ms over 1440 rows, and Independent Flip held for 98% of presents (PresentMon).
- **The raced presenter** (brief 3): slices presented ahead of the beam, scheduled from the pace hint
  and the observed arrival slope, with a measured margin. 2965 of 3470 frames raced in the first run,
  late slices under 0.1%.

The gate (short form, 60 s runs, ABBA, two runs a side, the first 300 frames of each run dropped,
raced against whole, `NIGHTJAR_BEAM_RACE_PRESENT=0` as the whole side). The model is a gain of
`min(W, S)` minus the margin, where `W` is the frame's time on the wire at the pace (7.4 ms at pace
900, 3.17 ms at 2100) and `S` the scan (4.17 ms). The keep rule asks for a gain of at least
`min(W/2, S/2)` on the mid-row age (when the beam reaches the middle row minus when the slice holding
it was ready), median and p95, with `receive_to_present` p95 no worse, late slices under 1%, no
composed presents and no hang.

| Leg | Required gain | Mid-row age p50 whole -> raced | p95 whole -> raced | `receive_to_present` p95 whole -> raced | Late slices | Frames raced |
|---|---|---|---|---|---|---|
| 1: pace 900 | 2.08 ms | 5.32 -> 3.93 ms (gain 1.40) | 5.98 -> 5.33 (gain 0.65) | 8.64 -> 8.61 | 0.061% | 5301 of 6578 |
| 2: pace 2100 | 1.58 ms | 4.18 -> 3.47 ms (gain 0.71) | 4.79 -> 4.19 (gain 0.60) | 4.59 -> 5.19 (worse) | 0.000% | 6344 of 6582 |
| 5: pace 900, 0.1% loss | (leg 1's) | 5.30 -> 3.91 ms (gain 1.39) | 5.88 -> 5.45 (gain 0.44) | 8.33 -> 8.53 | 0.015% | 2646 of 3292 (one run a side) |

Every run exited 0 with no hang, no dropped frames and no decoder errors, and PresentMon counted the
same handful of composed presents on both sides (4 to 6 `Composed: Flip` a run at the start, and the
overlay's 20 to 25 `Hardware Composed`). Leg 1 and leg 2 miss the rule on both the median and the p95,
leg 2 also on `receive_to_present`, so it does not become the default. Legs 3, 4 and 7 (the NUC, 2.5 GbE, a
windowed run) were not run.

Why the gain is small. Only the mid-row of the picture is better; the rest of the frame is not. The
last rows reach the screen later than a whole present puts them (extra draws per slice, and a final
draw of the remaining rows), `receive_to_present` p50 is 0.5 ms worse at pace 2100, and the overlay,
when on, costs about 0.2 ms per present (egui draws it on every present, a raced frame has 2.4 of
them). A frame is raced only when its slices arrive over more than about 1 ms, and the host sends
in 1 ms batches, so at pace 2100 (W = 3.2 ms) there is little left to win. A margin sweep was not run.

Tried to cut the cost of a raced frame (a scissored draw per slice, a GPU sync per slice, a raw
`Present1(0, ALLOW_TEARING)` instead of `SDL_RenderPresent`), keeping what measured lower: drawing
consecutive slices of one plane set in a single pass and presenting a finished picture before starting
another slice helped a little and is in the gate numbers; the raw `Present1` and skipping the flush did
not move them.

Caveats on the numbers. Loopback puts the host's encoder and the client's decoder and present on the one
GPU, so a raced frame competes with the encode of the next one; and the host sends in batches, so
slices arrive in steps of about 1 ms instead of continuously. Both make the race look worse than it
would on a separate client over a wired link, which is the case the idea is for; that run (the NUC,
1 GbE and 2.5 GbE) is not done, and the leg here reads `receive_to_present` over loopback only. The
number to beat on that link is the same rule.

Decision: kept as an opt-in, off by default, not shipped as the default. The median picture age
gains 1.4 ms at pace 900 and 0.7 ms at pace 2100, which is less than the rule asked for, and the p95
gains little. The wired NUC legs (3, 4 and 7), where the idea is meant to pay, are not run: until they
are, the setting stays off by default and nothing recommends it. To try it, set `beam_race` in the
settings file or run with `NIGHTJAR_BEAM_RACE=1`; `NIGHTJAR_BEAM_RACE_PRESENT=0` keeps the probe and the
slices but presents whole pictures (the measurement's "whole" side).

## 4:4:4 at 4K on a 780M: what the decode costs, and the Linux report it does not explain

A Linux report (Flatpak, KDE Plasma Wayland, Radeon 780M with RADV, 3840x2160 at 112 fps, PyroWave
10-bit HDR10 at 1.4 Gbps, GPU decode on the graphics queue) logged a `gpu` stage of 9.1 to 9.4 ms
(p50) and a `push` of 1.4 to 2.0 ms, against an 8.9 ms frame interval; the user said it is slow with
4:4:4 and fine with 4:2:0. The same GPU here, on Windows, 4K at 120 fps, 1.4 Gbps configured, 8-bit
SDR, an unchanged desktop scaled to the stream, 40 s legs, the released 4.12.3 build, isolated host:

| Chroma | Queue | `input_to_output_ms` p50 / p95 | push p50 | submit p50 | gpu p50 / p95 |
| --- | --- | --- | --- | --- | --- |
| 4:2:0 | graphics | 2.69 / 2.84, 2.68 / 2.83 | 0.34 | 0.26 | 2.07 / 2.14, 2.06 / 2.15 |
| 4:4:4 | graphics | 4.10 / 4.48, 4.03 / 4.31 | 0.38 to 0.39 | 0.27 | 3.41 / 3.50, 3.36 / 3.44 |
| 4:4:4 | compute | 4.02 / 4.41 | 0.37 | 0.27 | 3.36 / 3.45 |

4:4:4 decodes 1.65 times as long as 4:2:0, not twice: the entropy work follows the bitrate, only the
wavelets follow the samples. Nothing in the client's 4:4:4 path differs beyond the plane sizes (full
size chroma planes, the SDK decoder made with `chroma = 1`); there is no extra pass or fallback.
10-bit could not be streamed here (PyroWave has no 10-bit SDR mode, and the laptop's panel is not in
HDR), so the SDK alone measured it (`pyrowave_sdk::tests::decode_timing_at_1080p_1440p_and_4k`,
synthetic records, its own GPU timestamps on the 780M): 4K 4:2:0 2.18 ms, 10-bit 2.26 ms; 4:4:4 4.07
ms, 10-bit 4.25 ms. 10-bit adds about 4 %. (The synthetic 4:4:4 frames carry twice the bytes of the
4:2:0 ones, so that ratio overstates what a stream at a fixed bitrate sees.)

So the reported 4:4:4 10-bit decode would take about 3.6 ms on this laptop under Windows; the report's
9.2 ms is about 2.6 times that, and its CPU-only `push` is 4 to 5 times ours. If 4:2:0 is slower on
that machine by the same factor (about 5.5 ms), it stays under the frame interval and looks fine, while
4:4:4 crosses it and the decoder queue fills: that fits what the user saw, but the factor itself is
not measured here. Candidates, none proven: the GPU and its shared memory saturated by the Linux HDR
path (the plane copy out of the decoder's images, three full-size planes in 4:4:4, the RGBA16F picture,
the back buffer and the PQ pass on the same graphics queue as the decode, plus the compositor's HDR
composition; a `gpu` stage close to the frame interval and a slow CPU `push` both fit memory
contention), the GPU held at low clocks (power profile or TDP limit), or the driver's code for the
SDK's shaders. On Linux the `gpu` stage is wall time on the wait thread and so includes waiting for the
queue.

What the log now says, to tell these apart (each at the stream's end):

- The streamed codec names its chroma: `PyroWave 4:4:4`, `PyroWave 10-bit 4:4:4`. Before, the log and
  the overlay said `PyroWave 10-bit` for both chromas.
- On Linux the decode's `submit` stage is the time until the SDK's submit returned (which includes the
  wait for the renderer's queue lock) and `gpu` the rest; before, `submit` read 0 and `gpu` held both.
- `PyroWave GPU passes (SDK GPU timestamps, ms per frame): ...`: the SDK's own GPU time for the
  stream. Far below the `gpu` stage means the decode waited on a busy queue; close to it means the
  GPU itself is slow.
- `GPU during the stream: amdgpu cardN over N samples a second apart: shader clock p50 ... MHz
  (lowest ...) of ..., memory clock ..., busy p50 ... %, performance level ..., platform profile ...`
  (Linux, amdgpu, `stream/gpu_clocks.rs`): low clocks with a busy GPU point at power limits; high
  clocks with a busy GPU at saturation.
