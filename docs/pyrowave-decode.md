# PyroWave GPU decoding in the desktop client

Status: the SDK binding and its packaging exist (brief G1) and are measured; nothing in the stream
calls them yet (brief G2 wires them in). The CPU decoder in `stream/pyrowave.rs` stays the only
decoder until then, and stays as the choice of anyone whose GPU or driver fails the probe: the
client never falls back to it silently, it says why the GPU decoder is unavailable.

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
