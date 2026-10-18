# Streaming stability audit, October 18, 2026

[Architecture](architecture.md) · [Release notes](../rust/RELEASE_NOTES.md)

This audit starts from `fa5b26cef28474cb114e4e7879057e030036fb95`
(2.5.5). It investigates recurring AMD device resets and reviews the ownership,
synchronization and recovery code around a streamed frame.

## Reproduced defects and corrections

### Shared textures could be returned after failed ordering

`Returns::hand_back` called `ReleaseSync` even when the preceding D3D12 signal
or D3D11 wait failed. A failed ordering call does not establish that GPU readers
have finished. Returning that texture could let its producer overwrite it.

The return now stops at the failed operation, retires the compute queue and
leaves the failed texture owned. Acquiring or returning another texture on
that retired queue requires capture to reopen. Successfully enqueued work is
still flushed. Existing queue retirement and capture recovery provide the
replacement device; successful returns keep their asynchronous ordering.

The fault injection regression checks successful return and failures of signal,
wait and release. It verifies that a failed signal/wait never reaches release,
and that a retired queue cannot execute a subsequent return.

Source: [shared texture return](../rust/windows/src/compute.rs).

### AMF keyframe fallback used the global input counter

When the driver omitted its output-type property, output was labelled as an
IDR only when the encoder's input counter equalled one. Buffered startup,
recovery and encoder recreation can return a keyframe at a different counter.
The fallback now uses the effective IDR request stored with the output's
matching submission timestamp. A valid driver-reported type takes precedence.

Surface preparation can also force an IDR when long-term-reference properties
are rejected. That effective request now reaches submission tracking and
keyframe acknowledgements in both CPU and GPU encoding paths.

The regression covers initial, buffered and continued-index recovery requests,
normal pictures, explicit driver reports and missing submission metadata.

Source: [AMF output and submission tracking](../rust/windows/src/amf.rs),
[regression](../rust/windows/src/amf/tests.rs).

### Uploaded cursor textures lacked their shader-read transition

A plain cursor texture was implicitly promoted from `COMMON` to `COPY_DEST`
by its upload on the compute queue, then sampled as a shader resource. This
texture does not qualify for automatic decay after that write.
[Microsoft's resource-state rules](https://learn.microsoft.com/en-us/windows/win32/direct3d12/using-resource-barriers-to-synchronize-resource-states-in-direct3d-12)
require a transition before reading it.

The upload now records `COPY_DEST -> NON_PIXEL_SHADER_RESOURCE` once per new
shape. Cached cursor use adds no CPU wait or per-frame transition.
The opt-in native regression uses Direct3D's independent resource-state
assertion after GPU completion. Two different 9×5 shapes also exercise padded
upload rows and cache reuse.

| Native check on RX 7900 XT, driver 32.0.32015.2008 | Before | After |
| --- | ---: | ---: |
| Uploaded shapes reported shader-readable | 0 / 2 | 2 / 2 |
| HEVC/AV1 SDR/HDR with cached, replaced and hidden cursors | not rerun | 0 Direct3D errors |
| Independent strict software decoding, four cases | not rerun | 128 / 128 frames, no errors |

The combined fixture uses 32 synthetic 3840×2160 inputs per case at a configured
119.88 FPS, submits one input at a time, and includes two cursor shapes and
eight hidden-cursor frames. FFmpeg uses `hevc` and `obu` demuxers, error
escalation and frame hashes. These are correctness checks, not throughput,
picture-quality or latency measurements.

The initial cursor diagnostic had its own validation-operation lifetime error.
The harness was corrected to wait for a fence after the queued state assertion
before replacing a shape. The before/after results above both use the corrected
harness. That diagnostic process failure is separate from the streaming reset.

Source: [cursor upload and native assertion](../rust/windows/src/compute.rs),
[codec/cursor fixture](../rust/windows/src/amf/intra_refresh_native_tests.rs).

## The additional 2.5.5 reset remains unexplained

At October 17, 22:17 UTC, an installed 2.5.5 session stalled after approximately
16 minutes of 4K119.88 HEVC HDR streaming. It used the display driver's standard
keyed-mutex ring, D3D12 AMF input and the RX 7900 XT. Experimental fence transport
was off. Before the stall, the log reported approximately 120 FPS,
0.3 ms conversion and 6 ms encoding.

A trivial GPU query stalled for about three seconds. A later `AMF SubmitInput`
returned after 34,624 ms with `DXGI_ERROR_DEVICE_RESET`; encoder/context
termination then took about six/eight seconds. Windows left the adapter
unavailable, reporting Code 22 and subsequently Code 31. Restarting Windows
restored Code 0. The host service and status endpoint remained available.
Two Windows watchdog dump files were empty, preventing analysis of the
initiating GPU operation from those files.

This incident establishes that 2.5.5 did not resolve the recurring reset on
this host. It does not identify faulty hardware or prove that any of the
three defects above initiated this reset. Short native checks cannot establish
long-session stability.

Native vendor calls can block beyond the host's recovery deadline. The launch
guard waits while previous resources are still owned; bypassing it by freeing
live GPU allocations would be unsafe. Neither that limitation nor the origin
of the repeated resets is resolved by this audit.

## Coverage and verification

The production implementations of compute conversion/shared returns, AMF and
its GPU wrapper, the video sender/transport, device-loss classification,
capture publication, driver frame-ring submission, and stream device recovery
were read in full. Relevant sections of the larger capture bridge, driver ring
protocol, stream lifecycle/teardown, cursor recovery, PyroWave, WASAPI/Opus
audio and packet pacing/encryption paths were reviewed around their callers
and resource owners. This is a focused source audit, not a claim that every
line of the repository has been audited or that no bugs remain.

- `cargo test --workspace --locked`: 1,201 passed, 72 opt-in tests ignored.
- Final Windows library suite: 320 passed, 64 ignored.
- Native cursor assertion: expected baseline failure, corrected implementation passed.
- Native codec/cursor fixture: four cases, zero Direct3D errors, all 128 frames decoded.
- Workspace/all-target Clippy with warnings denied, formatting and diff checks passed.

Raw host logs, private Windows diagnostics, before/after reports, encoded
streams and decoded frame hashes are retained locally under
`D:\CodexArtifacts\rubylight-stability-audit-20261018`.
