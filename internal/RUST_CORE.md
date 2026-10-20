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
| `nightjar-cli` (planned) | `protocol/cli` | Headless test client: pair, launch, stream, decode-count, report. Drop-in for Rubylight's `rust/tests/moonlight_client.c` so Rubylight's `interop.py`/`e2e.py` drive it unchanged. |
| desktop client (planned) | `desktop/` | Windows and Linux app on `nightjar-core`. |

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
- `session`: wires the above to sockets and threads; the public API.

## Verification

1. Unit tests per module.
2. Differential tests against the C it replaces: the C sources are compiled in tests (`cc`
   crate, dev-only) and fed the same packet sequences, including loss, reordering and
   duplicates; outputs must match exactly. Rubylight's `rust/tests/fec-reference` does the
   same for FEC.
3. End to end against an isolated Rubylight host (`rust/release/e2e.py`, `e2e_host.py`) with
   `nightjar-cli` in place of the C test client.
4. On Android, the Rust engine ships behind a setting first, then becomes the default, then
   the C is removed.

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
- [ ] `session` + `nightjar-cli`, e2e against Rubylight
- [ ] Android switch-over behind a setting
- [ ] Desktop client (Windows, Linux)
