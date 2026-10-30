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
keyed-mutex ring, D3D12 AMF input and the RX 7900 XT. The opt-in fence transport
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

## Display, power and scheduling follow-up

The follow-up starts from `ce3292ab195f97140aa3fc6507cb0b4270d2cf80`.
The original report mixed confirmed defects with already-corrected behavior
and possible causes that have not been reproduced.

- Display restoration defaults on after three seconds. Exclusive layouts
  release after that delay even in older profiles that disabled restoration.
  Layouts that disabled an original monitor restore it after local Win+P
  changes too. A failed restoration remains journaled, and a later acquisition
  or failed-acquisition rollback uses the original baseline. Explicit
  `virtualDisplay=0` selects a physical display, including retained-session
  matching. Eligible device layout preferences are recorded at the last
  stream's disconnect, not when a paused lease is eventually destroyed.
- IddCx adapter and monitor identities survive sleep; swap-chain workers,
  frame rings and pooled graphics resources are retired before power-down.
  True PnP teardown clears the adapter and render pin. Initialization
  completion is matched to its returned adapter, including early and stale
  callbacks. Host recovery may request a restart only for one verified,
  stopped Rubylight devnode with Code 10, 31 or 43. It excludes healthy,
  manually disabled, ambiguous and unrelated devices, and bounds retries.
- Non-NVIDIA adapters use high rather than realtime process GPU priority.
  A retired compute queue rejects new conversion, cursor and capture work;
  already-admitted operations retain their ownership and completion fences.
  Retirement does not wait behind native GPU calls. These changes do not
  identify the cause of the long-session AMD reset.
- Packet, encoder and frame waits use the high-resolution timer throughout.
  Notification-capable capture waits longer after 100 ms without a frame,
  while active capture and pending transfers keep their short polls. Media
  thread scheduling remains enabled; process-wide high CPU priority is no
  longer requested by the host or capture helper.

The existing keyframe-loss pacing guard was already enabled. Host-adaptive
pacing remains opt-in. Capture recovery already waits for
encoder consumers before reopening, and native GPU resource destruction
already occurs outside the capture-state lock; those paths were not replaced.

The additional input review confirmed that unknown-contact moves and failed
hover/move injections could leave invalid touch state for later refreshes.
Only accepted frames now update that state. Failed batch cancellation tries
each contact and retires the injector-owned synthetic device if cleanup still
fails. Mouse/key teardown retries the final release once under the existing
shared-ownership lock, reports failure and continues other cleanup. These are
confirmed code defects; they do not by themselves reproduce the dragging
reported in issue #35. Pure injected-failure tests do not send input to the PC.

Compute conversion also validates the resource state reported by AMF before
recording a transition. Unsupported video/graphics states and invalid mixed
read/write masks fail explicitly. No native observation established that AMF
returned such a state during the reset. The surface-address callback concern
was not reproduced: AMF specifies notification before releasing its internal
resources. The compressed-frame copy remains necessary because the sender
must own its bytes after releasing AMF's output buffer. Repeated control-loop
session snapshots and gamepad batches now retain their allocated buffers.

The updated driver was built with MSVC and the Windows Driver Kit but was not
installed during verification. Actual sleep/wake, local monitor restoration,
and a sustained game/client stream remain hardware validation requirements.
Short synthetic checks cannot establish AMD reset prevention or game smoothness.

The [timer comparison](../rust/PERFORMANCE.md#october-18-timer-waits-and-idle-capture)
measured 97.6–99.1% fewer execution cycles for packet/frame waits, with mean
deadline lateness increasing to about 0.25–0.29 ms. The
[GPU comparison](../rust/PERFORMANCE_WORK.md#october-18-retired-compute-queues-and-conservative-gpu-priority)
completed all 2,048 readback-checked samples; noisy baseline tails prevent a
latency-improvement claim. Both records retain the methods, raw evidence
locations and limits of the measurements.

Follow-up verification:

- `cargo test --workspace --locked`: 1,225 passed, 74 opt-in tests ignored.
  This includes the actual driver lifecycle policy, paused-layout preference
  boundaries, display recovery journal retention, input failure handling and
  retained gamepad buffers.
- Both new opt-in native GPU regressions passed independently of the suite.
- The driver built with MSVC/WDK. Driver Clippy completed with the existing
  `linker_messages` allowance; five pre-existing runtime-signature warnings
  remain in generated `wdk-sys` bindings. Staged source hashes matched the checkout.
- Console type/Svelte checking reported zero errors and warnings; all 29
  settings/control tests passed.
- Workspace/all-target Clippy with warnings denied, formatting and diff checks passed.

Follow-up build logs, driver evidence and raw CPU/GPU measurements are retained
locally under `D:\CodexArtifacts\rubylight-four-fixes-20261018`.
