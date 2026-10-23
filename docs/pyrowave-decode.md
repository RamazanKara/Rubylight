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
- One shared `ID3D11Fence` is imported as a timeline semaphore. A decode signals `counter + 1`; the
  presenter queues `ID3D11DeviceContext4::Wait(fence, value)` before it samples the planes.
- A frame is `clear`, one `push_packet` per record (the sequence header record and the block
  records; the SDK accepts them in any order), `is_ready`, `decode(slot)` (acquire from
  `QUEUE_FAMILY_IGNORED`, release to `QUEUE_FAMILY_EXTERNAL`, signal the fence), then the D3D11 wait.
  `wait(value)` is a CPU wait on the same fence, for tests.
- Records come from `rubylight_protocol::pyrowave::Records`: `push_frame` for a whole frame, or
  `begin_frame` / `feed_packet` / `drain_new` / `finish_frame` to push records as they arrive.

### Probe and errors

`pyrowave_sdk::probe(device)` builds a minimal decoder, so every requirement is exercised for real.
The error names the first missing piece: the DLL (path searched), its ABI (`found 2.1.4, this client
needs major 1`), `ID3D11Device5` (shared fences, Windows 10 1703), a Vulkan device for the adapter
(with `diagnose_vulkan`'s account of the Vulkan loader and the adapters it saw), Vulkan 1.3 with
subgroup size control and the subgroup operations, D3D11 interop, plane import, or the fence. These
strings are unit tested with the DLL absent, with a DLL that is not the SDK, and with a fake DLL of
the wrong ABI.

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
`libpyrowave-shared-1.dll`, `build-info.txt` (pin, patches, SHA-256, toolchain) and `licenses\`.

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

| Size | Frame bytes | CPU push + submit p50 / p95 | Submit to signalled p50 / p95 | GPU p50 / p95 |
| --- | --- | --- | --- | --- |
| 1080p | 1.12 MB | 0.214 / 0.285 ms | 0.375 / 0.444 ms | 0.059 / 0.060 ms |
| 1440p | 1.95 MB | 0.321 / 0.432 ms | 0.538 / 0.642 ms | 0.081 / 0.084 ms |
| 4K | 4.28 MB | 1.763 / 2.394 ms | 2.124 / 2.880 ms | 0.144 / 0.156 ms |

GPU time is the SDK's own per-pass report (dequantisation plus inverse wavelet). The CPU side is
the cost, and it is mostly the SDK copying records in `push_packet`; it scales with the bytes.

### The Vulkan to D3D11 handoff

The time from the SDK's work finishing (timeline value signalled) until D3D11 can sample the planes.
There is no copy on the D3D11 side. What D3D11 pays is the fence wait, measured as a queued
`Wait(fence, value)` plus an event query, minus the same event query on an idle queue:

| Size | Wait adds p50 / p95 | Submit to D3D11-done, no CPU wait p50 / p95 |
| --- | --- | --- |
| 1080p | 0.039 / 0.077 ms | 0.387 / 0.474 ms |
| 1440p | 0.043 / 0.090 ms | 0.558 / 0.635 ms |
| 4K | 0.041 / 0.084 ms | 2.027 / 4.428 ms |

The handoff is about 0.04 ms at the median and under 0.1 ms at p95 at all three sizes, so below the
0.2 ms line; the 4K total is the push cost of a 4 MB frame, not the handoff. A D3D12 presenter
would not remove anything measurable here.

## Accuracy and handles

- The four fixtures (`pyrowave-{420,444}-{8,10}`) decode on the GPU to within 1 code value of the
  `.yuv` once rounded to a code (unrounded max 1.000, 1.033, 1.000, 1.154; the 10-bit planes differ
  by the half code of the SDK's float arithmetic) and within 0.89 of the CPU decoder (0.674, 0.826,
  0.625, 0.888).
- Lossy frames (lost block ranges, whole frame or records pushed early) match the CPU decoder
  within 0.68 code values.
- Ten decoder lives (create, decode, drop) leave 1 handle in total with a shared device that
  flushes, none with each life's own device, and none when created and dropped without decoding.

## Tests

`cargo test -p nightjar-desktop` runs the layout, ABI and probe-string tests. The GPU tests are
`#[ignore]`d (they need the DLL from `build-pyrowave-sdk.ps1`, a GPU, and no other GPU work):
`cargo test -p nightjar-desktop --release --bin nightjar -- --ignored --nocapture --test-threads=1
pyrowave_sdk::tests`.
