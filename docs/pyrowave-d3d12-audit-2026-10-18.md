# PyroWave and D3D12 audit follow-up — 18 October 2026

This follow-up addresses the later PyroWave and D3D12 audit. The separate
[display, driver and scheduling work](stability-audit-2026-10-18.md) is preserved
from `main` and is not attributed to this change.

## Changes

| Area | Result |
| --- | --- |
| PyroWave pacing | Host Wi-Fi and unmeasured client paths use conservative bursts. Explicit limits, the client's calibrated pace and endpoint link limits remain ceilings even when a stream cannot fit; the stream then warns about insufficient capacity. |
| Loss feedback | PyroWave participates in the existing loss guard, optional packet/bitrate adaptation and adaptive error correction. The optional controls keep their existing defaults and ownership rules; a client bitrate change still takes control from host adaptation. |
| Frame budgets | Both framing modes use one nominal frame allowance after a pause. Adaptive bitrate keeps enough budget to encode a frame, including the configured parity and negotiated minimum, without raising an impossible user limit. |
| Legacy error correction | Older length-framed clients receive ordinary Moonlight parity where their four Reed–Solomon blocks can carry it. Large frames reduce or omit protection and produce a warning. Parity consumes picture budget within the previous wire allowance; zero retains the unprotected format. Record clients keep selective coarse/detail protection. |
| Calibration | Probe and streaming use the same segmented UDP sender, bounded retries and burst limits: one millisecond, 64 packets and 65,507 payload bytes, allowing at least one packet. Overdue probes do not accumulate catch-up bursts. |
| Vulkan failure | The host checks the release timeline before reading encoded output. The pinned SDK signals it after its output copies. An error cannot queue an unsignalled D3D11 capture wait. Unconfirmed work retains its runtime, images, converter and capture lease; further PyroWave work on that adapter is blocked until host restart. |
| D3D12 ownership | Fence values are associated with the exact queue timeline. Failed signals and timeout cleanup retain command storage, textures, fences and capture leases until a completion marker or confirmed device removal makes release safe. New compute generations wait for retained work to drain. |
| Conversion and AMF | SDR BT.2020 applies the gamut and transfer conversion; standard 4:2:0 uses the signalled left chroma position, while PyroWave remains centred. PQ input is rejected for SDR. Planar write support is checked before AMF chooses D3D12. Released targets receive bounded completion waits, and capacity waits use notifications or bounded sleeps. |
| Cursor shapes | D3D11 and D3D12 reuse immutable animation shapes. Each cache keeps at most 16 entries and 4 MiB of pixel payload; queued frames and uploads retain evicted resources until completion. |

## Compatibility and limits

Legacy parity uses Moonlight's existing transport reconstruction before PyroWave
depacketization; it does not require record framing or a new client protocol.
The client reference is pinned to Nonary's `moonlight-common-c`
`d6a11bc685b41037b352a96f29d08276fe5359ba`, with PyroWave SDK `502a3b52` and the
original nanors reference used for independent parity checks.

Adding parity trades some legacy picture quality for recovery at the same
network allowance. The native conversion test's near-lossless control uses
2 Gbps to leave room for that protection; its picture-quality thresholds are
unchanged. The lower-rate cases remain 400, 125 and 30 Mbps.

Encoding remains synchronous. Using both SDK frame contexts would require
changing the frame-delivery pipeline and evaluating added buffering and latency;
this audit does not claim a throughput improvement from asynchronous encoding.

The timeout tests gate only task-owned queues and then release them. They do not
trigger a GPU reset. Vulkan error classification and retention are covered by
regressions without deliberately causing a real device loss. These fixes do not
establish the cause of the reported AMD reset, long-session stability, or
performance on a changing Wi-Fi connection.

## Verification

The final source includes the separate stability work through `dc4c705d`.
Native GPU checks ran on the AMD Radeon RX 7900 XT.

- `cargo test --workspace --locked -- --test-threads=4`: 1,251 passed,
  none failed, 81 hardware/manual tests ignored by the ordinary suite.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` and
  `cargo fmt --all -- --check`: passed.
- Settings schema: all 19 checks passed.
- Native colour and cursor checks: 13 passed, including 960 video/planar
  conversion variants across CPU, D3D11 and D3D12. Independent references
  checked SDR BT.2020, chroma position, wide-gamut FP16 input, ranges,
  scaling and cursor composition. Repeated cursor shapes reused resources.
- Native compute/capture ownership: 10 distinct tests passed in 13 runs.
  Four checks were repeated after integrating the separate stability work,
  covering unsupported AMF states, converter and handoff teardown after
  timeout, and exact fence identity. The other checks covered released
  targets, retired queues and capture ownership.
- PyroWave native decoding: all 1,152 frame comparisons passed across
  1080p/720p, SDR/HDR, 4:2:0/4:4:4, legacy/record framing and four bitrate
  controls. Graphics and compute paths matched exactly within each framing
  mode, repeat frames stayed identical, and the existing quality thresholds
  passed. The rebuilt-plane check passed another 12 frames across seven
  source-format changes and six reused image handles.
- Independent transport fixtures: all 24 passed. Original C++ nanors parity
  matched byte for byte; deliberately dropped data shards were recovered to
  the original payload; the vendor decoder accepted every legacy/record,
  SDR8/SDR10/HDR10, 4:2:0/4:4:4 and plain/encrypted combination. The new
  offline decoder builds with `-Wall -Wextra -Werror` and uses eight-bit SDK
  CPU readback; full HDR precision is covered by the native GPU matrix.

`rust/tests/build-pyrowave-client.ps1` retains its live streaming client and
also builds `pyrowave-decode-probe.exe`. Pass that separate offline executable
to `rust/tests/pyrowave_transport.py` with the generated fixture directory
and original nanors reference DLL.

The hardware checks use uploaded test images and isolated queues. They do
not install a new host or driver, alter displays, or constitute a live-client
soak test.
