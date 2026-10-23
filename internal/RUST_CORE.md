# Rust streaming core

Nightjar is moving its streaming engine from moonlight-common-c (C, plus Nightjar's patches and
`android_*.c` replacements) to Rust, so one implementation serves the Android app and the desktop
clients for Windows and Linux. This file is the plan and the status; update the status as work
lands.

## Layout

The Rust workspace is `protocol/`:

| Crate | Path | Role |
| --- | --- | --- |
| `rubylight-protocol` | `protocol/core` | `no_std` wire formats shared with the Rubylight host (which depends on it by git revision). Keep it dependency-free. |
| `nightjar-core` | `protocol/client` | The client streaming engine (std): pairing and host HTTP, RTSP/SDP, the encrypted ENet control stream, input, video and audio receive with FEC, PyroWave records, timing. No platform code: decoding, rendering, audio output and input capture belong to the apps. |
| `nightjar-difftest` | `protocol/difftest` | Test-only (`publish = false`): builds the patched moonlight-common-c receive paths in `build.rs` (sources copied to `OUT_DIR`, patches applied there) and compares them with `nightjar-core` on identical packet sequences. Needs the `moonlight-common-c` and `nanors` submodules and OpenSSL; compiled out elsewhere. |
| `rubylight-protocol-ffi` | `protocol/ffi` | C ABI for the Android app. Grows a Limelight-compatible API over `nightjar-core` for the switch-over. |
| `nightjar-android` | `protocol/android` | `libnightjar_core.so` (`publish = false`): the JNI side of the app's `RustBridge`, built by Gradle's `buildRustEngine` for the four ABIs and not committed. See "Android switch-over" below. |
| `nightjar-cli` | `protocol/cli` | Headless test client (`publish = false`): stream, decode every frame, report. Drop-in for Rubylight's `rust/tests/moonlight_client.c` (same arguments, environment, output lines and exit codes) so Rubylight's `interop.py`/`e2e.py` drive it unchanged; pairing and launch stay in `interop.py`. Decoding needs the `ffmpeg` feature (libavcodec and libopus, as the C client). |
| `nightjar-desktop` | `protocol/desktop` | Windows and Linux client (`publish = false`, not a default member): SDL3 window and renderer, egui launcher, FFmpeg D3D11VA/VAAPI decoding, PresentClock pacing, SDL3 audio and gamepads. See `protocol/desktop/README.md`. |

## Module boundaries in `nightjar-core`

Each module is a pure state machine where possible (bytes and times in, events out), so it is
testable without sockets and without a host. Sockets and threads live in `session`.

- `crypto`: AES-GCM/CBC helpers used by control, input, audio and video encryption.
- `fec`: Reed-Solomon over GF(2^8), bit-compatible with moonlight-common-c's nanors.
- `video`: RTP reordering, FEC block recovery, frame reassembly (H.264, HEVC, AV1, PyroWave),
  loss and RFI reporting decisions. Mirrors `RtpVideoQueue.c` and `VideoDepacketizer.c` plus
  Nightjar's `android_rtp_video.c`/`android_depacketizer.c` changes.
- `audio`: RTP audio queue with FEC, mirroring `RtpAudioQueue.c`; yields Opus packets.
- `control`: ENet (rusty_enet, which the Rubylight host also uses) and the encrypted control
  protocol: pings, loss stats, IDR/RFI requests, HDR, rumble, triggers, haptics, Rubylight's
  0x55xx messages, termination.
- `input`: input packet encoding (keyboard, mouse, controllers, touch, pen, motion, battery).
- `rtsp`, `sdp`: the RTSP handshake and the stream description, mirroring `android_rtsp.c`
  and `android_sdp.c` (which already replace the core's versions).
- `http`: serverinfo, pairing, app list, launch/resume/cancel (on Android this is Java today).
- `session`: wires the above to sockets and threads; the public API (below).

## Session API

`Session::start(&HostInfo, SessionConfig, Box<dyn VideoDecoder>, Box<dyn AudioRenderer>,
Arc<dyn Listener>, &Interrupt)` is LiStartConnection: it reports each `Stage`, resolves the host
(STREAM_CFG_AUTO through `Location::Auto`, with the 1024/1184 packet caps), runs the RTSP
handshake (audio socket and pings from SETUP audio on), connects the control stream and starts
the video, audio and microphone paths. `Session::stop` (or drop) is LiStopConnection, in the C's
order; terminations after it are not reported.

- `VideoDecoder`: `setup`, `start`, `submit(&Frame) -> DecodeStatus::{Ok, NeedIdr}`, `stop`,
  `cleanup`. `CAPABILITY_DIRECT_SUBMIT` in the stream's video capabilities decodes on the
  receive thread; otherwise a decoder thread drains the 15-frame queue.
- `AudioRenderer`: `init(&AudioSetup)`, `play(Option<&[u8]>)` (`None` = conceal a lost packet),
  through the 30-packet queue unless the audio capabilities have direct submit.
- `Listener`: stages, `connection_started`, `connection_terminated(code)` (once, own thread),
  `control_event(ControlEvent)` for status, rumble, triggers, LEDs, motion, HDR, haptics, host
  timing, clock sync and cursor shapes, and `log`.
- `Session`: `input(|InputQueue| ..)`, `request_idr_frame`, `send(ClientMessage)` (0x55xx),
  `send_clock_sync_probe`, `send_microphone`, `hdr`, `stats` (RTT, video/audio counters,
  decode and queue counts, connection status), `outcome` (the negotiated stream).

Not ported: the pull renderer (`CAPABILITY_PULL_RENDERER`, LiWaitForNextVideoFrame), STUN
(LiFindExternalAddressIP4), socket QoS (SO_PRIORITY / SO_NET_SERVICE_TYPE), GFE generations
before 3.22.

## nightjar-cli

Windows (GNU toolchain, MSYS2's FFmpeg and Opus, as Rubylight's C client):

```powershell
$env:PATH = "C:\msys64\ucrt64\bin;$env:USERPROFILE\.cargo\bin;" + $env:PATH
cargo build --release -p nightjar-cli --features ffmpeg   # in protocol/
```

Linux needs `libavcodec-dev` and `libopus-dev`; `NIGHTJAR_FFMPEG_DIR` overrides the prefix.
For `e2e.py`, put `nightjar-cli.exe` beside Rubylight's `audio_probe.exe` and `motion_probe.exe`
and pass it as `--client`; the package directory needs `butterpollo.exe`, its runtime DLLs and
`assets/web`. `e2e.py` expects the installed host running and idle on 47989.

E2E on 2026-10-20 (Radeon, AMF, Rubylight 2.5.9 at 49e7e3e5, 2560x720x60, 12 s): h264, hevc,
av1, h264 `--vrr` and h264 `--recovery 3` all passed, every frame decoded (679-707 frames, 0
failures, steady 60.1 FPS, audio tone continuous, 3/3 keyframe requests answered in 4.9 ms mean).
The C client on the same package: h264 710/710 and recovery 3/3 (5.2 ms mean), equivalent.

## Android switch-over

The setting Settings > Advanced > Diagnostics > "New streaming engine"
(`checkbox_rust_engine`, on by default since 2026-10-23; off selects the classic C engine) picks
the engine per stream:

- `NvConnection` calls `MoonBridge.requestRustEngine(prefs.rustEngine)` before
  `startConnection`. `MoonBridge` keeps its public API; every streaming call is now a Java
  method that goes to `RustBridge` when the current stream runs on Rust, else to the C native
  (renamed with a `c` prefix: `cStartConnection`, `cSendMouseMove`, ...). Pairing, HTTP,
  STUN, the connectivity test, controller guessing and the USB audio natives stay C/Java.
- `RustBridge.fallbackReason` keeps a stream on the C engine when the host is older than
  GFE 3.22 (app version below 7.1.431) or reports no codec support; a missing or broken
  `libnightjar_core.so` does the same. `StreamEngine` records the engine and the reason; the
  `Streaming engine` warning, the Statistics and Debug overlays (`... · Rust`, `Streaming engine:`
  line) and the problem report ("Streaming engine for the last stream") all show it, so a
  fallback is never only in logcat.
- `libnightjar_core.so` (`protocol/android`) calls the same `MoonBridge.bridge*` callbacks as
  `callbacks.c`, so the renderers, the audio renderer and the listener serve both engines
  unchanged. Opus is decoded in Rust against the app's prebuilt `libopus.a` (the archive
  `libmoonlight-core.so` links), so both engines decode with the same library and no cmake or
  second Opus build is needed; the microphone encoder uses it the same way.
- `connection_started` is forwarded after the session is stored, so Java can call into the
  session from that callback as with the C engine.
- Build: Gradle's `buildRustEngine` (app/build.gradle) runs `cargo build --release --locked`
  per ABI with the NDK's clang as `CC` and linker (`--target=<abi>21`,
  `-z max-page-size=16384`), checks every LOAD segment is 16 KiB aligned and adds the output
  as a generated jniLibs directory. It is up to date when nothing under `protocol/` changed.
  Not committed: about 1-1.5 MB per ABI stripped, and it changes with every engine change.

### Parity checklist

| Area | Rust path | Notes |
| --- | --- | --- |
| Start, stop, interrupt, stages, termination codes, host termination reasons | yes | Same stage numbers and `ML_ERROR_*` codes; stage names and port flags still come from the C helpers. |
| Network grace (`setNetworkGracePeriodMs`) | yes | `SessionConfig::network_grace`. |
| Input-only sessions | yes | `SessionConfig::input_only`. |
| Remote monitor sessions | yes | `SessionConfig::remote_monitor`: the input queue is disabled (nothing is sent, not even the haptics enable) and the start wiggle is skipped, as the C skips `startInputStream`. Server commands and controller audio messages are still sent on the control stream, as in C, which only skips the input stream. Loopback test `a_remote_monitor_connection_sends_no_input_at_all`. |
| H.264, HEVC, AV1, 10-bit, 4:4:4, RFI capabilities | yes | Parameter sets submitted separately, picture data in one buffer, timestamps mapped to CLOCK_MONOTONIC as `callbacks.c` does. |
| PyroWave | yes | The fallback is gone; a PyroWave stream ran on a phone in the B8 comparison (`docs/PARITY-MATRIX.md`). Records, SDP/RTSP and the queue are difftested against the patched C (`protocol/difftest/tests/video.rs`) and negotiated in `host_interop.rs`; the Java renderer receives the same `bridgeDrSubmitDecodeUnit` units. |
| HDR mode and metadata | yes | `SS_HDR_METADATA` layout. |
| Audio (Opus, surround, high quality, packet loss concealment) | yes | Decoded in Rust with the app's libopus. |
| Pending audio duration, pending video frames, RTT for the overlay | yes | From `Session::stats`. |
| Keyboard, text, mouse, scroll, controllers, touch, pen, controller touch, motion, battery | yes | `LI_ERR_UNSUPPORTED` and the other C return codes. |
| Rumble, trigger rumble, motion requests, LEDs, adaptive triggers, connection status | yes | |
| Controller haptics (0x5601), speaker and status (0x5533, 0x5535 in), microphone and status (0x5534, 0x5535 out) | yes | |
| Host timing (0x5536) | yes | Records re-encoded for `HostTimingReport.parse`. |
| Clock sync (0x5537) and PresentClock scheduling | yes | Probes stamped with CLOCK_MONOTONIC; replies feed the Rust `PresentClock`. |
| Phase lock reports (0x5530, 500 ms windows, newest only, 1 s max age) | yes | |
| Display caps (0x5531), reconfigure (0x5532), cursor shapes (0x5540/0x5541), server commands (0x3000) | yes | Cursor shapes re-encoded to the wire form for Java. |
| Microphone (Opus 32 kb/s VoIP, in-band FEC, 200 ms reset gap) | yes | Encrypted stream only, as in C. |
| GFE before 3.22, hosts without codec support | falls back | Not ported. |
| Socket QoS (always: `SO_PRIORITY` 6 audio and microphone, 5 video, 6 plus DSCP EF/ECT(1) on the control socket, dropped after three unanswered connects as ENet does) and "Prioritize stream traffic" (6 on the RTSP socket, nice -4 on VideoRecv, AudioRecv, AudioDec and the control thread; VideoDec is not raised, as in C) | yes | `nightjar_core::priority`, passed in `SessionConfig::network_priority` (no global state). Linux and Android only; a refused option is logged at warn through `Listener::warn`. Tests: `media_sockets_are_bound_with_their_priority`, `the_marking_comes_off_after_three_unanswered_connects` (Linux). |
| "Unbatched input" (no 1 ms mouse and pen batching) | yes | `SessionConfig::unbatched_input`, `InputQueue::set_unbatched`. |

Emulator test on 2026-10-20 (AVD nj-demo2, x86_64, Android 15, `c2.goldfish` decoders,
isolated Rubylight 2.5.9 host on this PC as 10.0.2.2, Desktop at 1280x720x60): Rust engine
H.264 and HEVC streams decoded and displayed at 59.5-60 FPS with 0% loss; the host logged the
display caps, host timing requests and phase lock reports from the Rust path, and the
disconnect was clean. The C engine on the same setup: 60 FPS, 0% loss, equivalent.

## Verification

1. Unit tests per module.
2. Differential tests against the C it replaces: the C sources are compiled in tests (`cc`
   crate, dev-only) and fed the same packet sequences, including loss, reordering and
   duplicates; outputs must match exactly. Rubylight's `rust/tests/fec-reference` does the
   same for FEC.
3. End to end against an isolated Rubylight host (`rust/release/e2e.py`, `e2e_host.py`) with
   `nightjar-cli` in place of the C test client.
4. On Android, the Rust engine shipped behind a setting first and is now the default (the C engine
   stays as the explicit opt-out); then the C is removed.

## Android parity checklist

Nightjar's native changes that the Rust engine must carry (see `app/src/main/jni/moonlight-core`
and its `patches/`): controller haptics and controller audio (0x5533-0x5535), host timing
(0x5536), clock sync (0x5537), cursor shapes (0x5540/0x5541), phase lock (0x5530), display caps
(0x5531), reconfigure (0x5532), network grace, host termination reasons, disconnect intercept,
control teardown ordering, PyroWave records and link hints, the larger video receive buffer,
microphone passthrough, STUN, the RTSP and SDP extensions, the bughunt fixes.

## Status

- [x] Wire formats crate, phase lock, PyroWave records, host timing, clock sync, PresentClock.
- [x] `nightjar-core` scaffold
- [x] `fec` + `video` (differential tests in `protocol/difftest`)
- [x] `audio` (differential tests in `protocol/difftest`)
- [x] `crypto` + `control` + `input` (encrypted control stream only; GFE's TCP and plaintext
  generations are not ported)
- [x] `rtsp` + `sdp` + `http` (TCP RTSP incl. encrypted; RTSP over ENet for GFE < 3.22 is not ported)
- [x] `session` + `nightjar-cli`, e2e against Rubylight (loopback session tests in
  `protocol/client/tests/session_loopback.rs`)
- [x] Android switch-over behind a setting (`protocol/android`, `RustBridge`; see the parity
  checklist and `docs/PARITY-MATRIX.md` for the remaining gaps)
- [x] Rust engine as the Android default (C engine kept as the opt-out)
- [ ] Removal of the C engine
- [x] Desktop client milestone 1 (Windows, Linux): pairing, library, launch/resume/quit, hardware decoding, VRR pacing, audio, input, overlay. Streamed against an isolated Rubylight host on Windows (HEVC D3D11VA up to 5120x1440@240). Next: decode-only FFmpeg for packaging, HDR output, VAAPI dmabuf import, a dedicated D3D11 renderer with a waitable swap chain, per-PC settings, Wake-on-LAN, OTP pairing.
