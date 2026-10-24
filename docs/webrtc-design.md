# WebRTC server design

Plan of record for issue #50: an optional WebRTC transport beside GameStream for the Moonlight WebRTC TV
protocol (Samsung Tizen TV app and the browser client). Target: Rubylight 3.0.0. Status lives in the
3.0 plan's status table.

## Pinned references

| What | Pin | Licence | Use |
|---|---|---|---|
| Protocol and TV app, [mlopezsegura/Monlight-Tizen-WebRtc-Client](https://github.com/mlopezsegura/Monlight-Tizen-WebRtc-Client) | `193ef539c3cf` (2026-10-19); `docs/protocol.md` last changed `c292eaf8c69e`, protocol version 2 | GPL-3.0 | The contract we implement |
| Browser client, [mlopezsegura/moonlight-webrtc-web](https://github.com/mlopezsegura/moonlight-webrtc-web) | `225a7ef54457` (2026-10-19) | none stated | Test client, fetched at test time and never vendored |
| Reference host, [mlopezsegura/Sunshine-Web-RTC](https://github.com/mlopezsegura/Sunshine-Web-RTC) | `2bc1989107c9` (2026-10-24) | GPL-3.0 | We read its source and docs only. We never build or run it, and we cite none of its numbers as ours |

All three repos are weeks old. Before each brief starts, and again before the release, compare
`docs/protocol.md` with the pin. If the protocol has changed, update this doc first.

## Decisions

1. **Separate transport, shared host.** WebRTC reuses capture, AMF encode, WASAPI and Opus audio, the
   gamepad worker, the paired-device list and permissions, the app library, and launch preparation and
   the session state machine. It adds a signalling server, discovery and a media sender. It never
   changes GameStream: RTSP, ENet, the GameStream packetizer, FEC, `video_send`, the core
   `packet` module and `PROTOCOL_VERSION` stay as they are.
2. **Off by default.** The `webrtc_server` setting (default `false`) gates everything. While it is off,
   no socket is opened, no thread starts and no GameStream branch changes behaviour. The console gets a
   toggle labelled "WebRTC server (Experimental)", plus port fields. Paired TVs and browsers appear on
   the Devices page with a "WebRTC" badge.
3. **Library: str0m 0.24.1.** Pin it as `str0m = { version = "=0.24.1", default-features = false,
   features = ["rust-crypto"] }`, in `rust/host` only. The reasons are in the next section.
4. **One stream on the host.** A WebRTC stream and a Moonlight stream exclude each other through the
   existing `Sessions` registry and `check_streaming_session`. A start that conflicts gets the
   protocol's error with a clear message. It never takes over silently.
5. **The host sends the offer** (protocol v2). The offer carries a single codec with fixed payload
   types. The host checks the answer and rejects any change of codec or profile with an error.

## Library

| | str0m 0.24.1 | webrtc-rs 0.21.1 | libdatachannel (datachannel 0.16 / -sys 0.23) |
|---|---|---|---|
| Model | Sans-IO: we own the UDP socket, the thread and the send timing | async wrapper over the new Sans-IO `rtc` 0.21 crates, tokio or smol | C++ library behind FFI, with its own threads |
| Toolchain | Built and produced an offer on 1.98.1 GNU (UCRT64) and 1.98.1 MSVC (probe, 2026-10-24); MSRV 1.85 | Not tried. The 0.21 line is a fresh rewrite | cmake plus a C++ build on both GNU and MSVC |
| DTLS/SRTP | `rust-crypto`: pure Rust (aes, aes-gcm, p256, sha2, DTLS from dimpl). `wincrypto` (Windows CNG) is the alternative to measure. No system dependency | ring or aws-lc-rs | OpenSSL: a new native dependency |
| Licence | MIT or Apache-2.0 | MIT or Apache-2.0 | MPL-2.0 |
| Dependencies | Small. Its RustCrypto crates sit beside the ones core already uses | Duplicates ours (rand 0.10, x509-parser 0.16 against 0.18), plus mdns, turn and a runtime | The whole native toolchain |
| Packetizers | H.264, H.265 (profile, tier and level), AV1, Opus | H.264, HEVC, AV1, Opus | H.264, H.265, AV1, Opus |
| Latency control | With no bandwidth estimate it uses a null pacer: a packet leaves when we poll. Writer playout-delay and abs-capture-time | Runtime scheduling sits between us and the socket | Its own send thread and queues |
| HEVC Main10 later | Profile-tier-level handling plus custom fmtp | fmtp strings | fmtp strings |

All three licences are GPL-3.0 compatible. str0m wins on control. The frame goes from the encoder
thread to our own socket without a runtime or foreign thread in between, so we can reuse our pacing
code and socket settings. It adds no native dependency and builds on both toolchains today.

What str0m needs from us:

- Clear the default codecs and set fixed payload types.
- Disable RTX, transport-cc and REMB in the offer.
- Add the playout-delay extension `http://www.webrtc.org/experiments/rtp-hdrext/playout-delay` to the
  extension map.
- Set the H.265 fmtp ourselves. str0m defaults to `profile-id=1;level-id=180`.
- Insert Samsung's `a=imageattr:<pt> send [x=[W:W],y=[H:H],fps=[F:F]]` line by editing the offer
  text. str0m keeps attributes it doesn't know, so the answer still parses.

Install the crypto provider once at startup with
`str0m::crypto::from_feature_flags().install_process_default()`.

## Architecture

```
TV / browser ──UDP 8000 discover──▶ webrtc::discovery
             ──WS  webrtc_port────▶ webrtc::server ── auth, pairing (h.pins, paired clients)
                                         │            apps (library), start/stop
                                         ▼
                              webrtc::session ── builds Launch + Negotiated, runs the nvhttp
                                         │        launch pipeline, then Media::start
                                         ▼
stream.rs spawn_session ── send_frames: `if let Some(w) = &webrtc_sender { return w.submit(..) }`
                          (next to the pyrowave_sender short-circuit, stream.rs:1609)
audio() ── Opus packet ──▶ webrtc sink (instead of the GameStream audio RTP)
                                         ▼
                              webrtc::media thread: str0m Rtc + UDP socket + pacer
                                         │  PLI/FIR, request-keyframe ──▶ Session.idr + recovery wake
                                         │  gamepad DataChannel ──▶ Input::Arrival / Input::Controller
                                         ◀──  rumble feedback ── control DataChannel
```

- **Code layout.** Protocol logic is pure and OS-independent, in `rust/core/src/webrtc/`. It needs no
  new core dependencies: HMAC comes from ring and constant-time compare from subtle. str0m, the
  sockets and the host glue live in `rust/host/src/webrtc/`.
- **Launch.** `nvhttp::start` is split into a front half (TLS client lookup, Moonlight query parsing,
  the reply) and a shared back half: superseding, teardown wait, `session_begin`, `replace_launch_app`,
  queue, `prepare_launch_display`, `prepare_launch_app`, `retain_launch_app`. WebRTC calls the back
  half with a client it has already authenticated and a mode taken from `start-session`.
  - The split must not change behaviour: the existing nvhttp tests stay green without edits.
  - GameStream's `Launch` fields (`key`, `ping`, `rtsp_*`) stay as they are. A WebRTC launch gets a
    random key that is never used.
  - A new `transport` field (default `GameStream`) tells the stream which senders to start.
- **Session.** RTSP builds `Negotiated` in ANNOUNCE. A new constructor,
  `Negotiated::webrtc(width, height, fps, codec, bitrate)`, builds it from `start-session` instead, with
  2-channel audio, 5 ms Opus packets, 1 slice, no FEC and no reference-frame invalidation.
  `Media::start` then runs unchanged. The WebRTC transport doesn't start the GameStream video, audio
  or ENet control sockets. The video peer that GameStream learns from Moonlight's UDP ping comes from
  the address ICE selects instead.
- **Video.** `webrtc_sender` is `None` unless the session is WebRTC. It hands each `Encoded` frame
  (Annex-B access unit) to the media thread over a bounded channel and returns.
  - The RTP timestamp is the 90 kHz value of the frame's capture instant, measured from session start
    and monotonic.
  - Each frame also gets a playout-delay of min=max=0.
  - A frame that misses its deadline is dropped and followed by an IDR request, the way GameStream
    recovery handles it. No queue builds up.
- **Audio.** `audio()` keeps its WASAPI capture and Opus encoder. The packet sink becomes a small
  trait: GameStream RTP/FEC (today's code, unchanged) or the WebRTC channel. Packets are 5 ms (240
  samples), stereo, 48 kHz, in their own media stream with a separate msid so the browser doesn't hold
  video back for lip sync. The fmtp line follows the protocol's
  (`minptime=20;maxaveragebitrate=128000;stereo=1;sprop-stereo=1;useinbandfec=0`) unless B2 shows a
  receiver rejecting 5 ms packets under it.
- **Input.**
  - `gamepad-connected` becomes `Input::Arrival` with the Xbox type; `gamepad-disconnected` clears the
    controller's bit in the active mask.
  - `gamepad-state` becomes `Input::Controller`:
    - Button bits are mapped to Moonlight's flags.
    - Triggers 0..1 are scaled to 0..255.
    - Sticks -1..1 are scaled to i16, with Y inverted.
    - Stale `seq` values are dropped. The 250 ms keepalive refreshes state but sends no change.
  - Events go into the same dispatcher the ENet control path feeds, which leads to the existing
    gamepad worker.
  - Rumble from `feedback_packets` (`RUMBLE_DATA`) is sent as `rumble` on the control channel, with
    magnitudes scaled to 0..1.
  - The protocol has no keyboard or mouse, so neither is in scope.
- **ICE.** The host is ICE-lite. Its only candidate is the local address the WebSocket arrived on,
  plus `webrtc_media_port`. There is no STUN or TURN.
  - Chrome hides its own host candidates behind mDNS names. We ignore them and learn the client's
    address from its connectivity checks (a peer-reflexive candidate).
  - A single UDP socket carries one session at a time.

## Security

- **Trust model.** A paired TV or browser is an ordinary `Client` entry in the paired list:
  - `cert` is empty.
  - `extra.webrtc` holds `{clientId, secret}`. The secret is stored as-is because the host computes
    the HMAC itself, and it sits in the same ACL-protected state file as the host key.
  - It has the same `perm` bits and `enabled` flag as any client.
  - Disabling the device answers `client-disabled`. Unpairing it answers `authentication-failed`.
    Either one drops its live WebSocket and stream.
  - At most 32 WebRTC clients are kept; the oldest is dropped first.
  - B3 checks every use of `Client.cert` so that an empty certificate can never match a TLS peer, and
    adds a regression test for it.
- **Pairing.**
  - `request-pairing` adds a `PendingPin` with an unguessable request ID and the client's name. It lasts
    5 minutes and allows one attempt.
  - The admin approves it through the console's existing PIN page, which calls `/api/pin` with
    `uniqueid`.
  - The host compares the PIN entered there with the one the TV showed, in constant time.
  - On success it issues a 128-bit `clientId` and a 256-bit secret from the OS RNG. On failure it
    answers `incorrect-pin`, `pairing-cancelled`, `pairing-expired` or `pairing-failed`.
  - `pair-client` gets `unsupported-pairing`.
- **Authentication.**
  - The nonce is 256 bits and single-use.
  - The proof is `HMAC-SHA256(secret, "moonlight-webrtc-tv-auth-v1:" + nonce_hex)`, compared in
    constant time.
  - At most 8 unauthenticated connections are kept (the oldest goes first). An unauthenticated
    connection can never displace a streaming one.
  - A paired client that reconnects replaces its own old connection.
- **What auth gives.** It stops other LAN devices from using or interrupting the stream. It does not
  make anything confidential: the WebSocket is plain `ws://`, and the secret is visible once during
  pairing. DTLS-SRTP encrypts the media itself. The console says this next to the toggle.
- **LAN only.**
  - Signalling and discovery accept only loopback, RFC 1918, link-local, ULA and CGNAT (100.64/10)
    peers. Anything else is closed before the first message and logged.
  - Both honour the host's bind address (`--bind 127.0.0.1` keeps test hosts local).
  - There is no option to widen this in 3.0.
- **Ports.**
  - The TV discovers on UDP 8000, so discovery always listens there, with `SO_REUSEADDR`.
  - Signalling uses `webrtc_port` (default 8000, TCP) and is advertised in the discovery reply.
  - Media uses `webrtc_media_port` (default 8001, UDP).
  - Port 8000 is often taken by dev servers. A bind failure is loud: a console warning
    (`webrtc_server`), a log error and a status of "not running". There is no silent move to another
    port.
  - B3 checks that the installer's firewall rules cover these ports.

## Latency

- No added latency on either path. With the setting off, the GameStream code runs exactly as before.
  The only new code on the hot path is one `None` check in `send_frames`.
- **Frame path.** The media thread wakes when a frame arrives and packetizes it at once through str0m.
  It drains `poll_output` straight into our pacer with no batching timer. `SO_SNDBUF` is 4 MB.
  - There is no BWE, so str0m's null pacer applies.
  - The pacer sends 1 ms groups at a rate above the stream's bitrate. B2 starts from the existing
    `network_pacing` frame-wire-rate maths and caps the rate at the routed link.
  - B5 A/B tests paced against unpaced on a 4K keyframe and keeps whichever measures better.
- **Bitrate.** Video gets the requested bitrate minus 128 kbps of audio and the RTP/SRTP overhead.
  Valid requests are 10,000 to 100,000 kbps.
- **Recovery.**
  - PLI, FIR and the control channel's `request-keyframe` set `Session.idr` and wake recovery, the same
    as a Moonlight IDR request.
  - There is no RTX in the MVP. B2 finds out whether str0m 0.24.1 can answer NACK without RTX. B5
    measures a 0.1 % loss leg with RTX on and off, and the better one ships.
- **No FEC and no reference-frame invalidation.** WebRTC has neither. Intra refresh stays off, as on
  GameStream.

## Scope

**MVP (3.0.0), labelled Experimental:**

- SDR H.264 at 720p, 1080p and 1440p.
- SDR HEVC at 1080p, 1440p and 4K.
- 30 and 60 fps. 90 and 120 fps are offered only for modes where B5 measures them clean.
- Opus stereo audio.
- Gamepads with rumble.
- Discovery, signalling, PIN pairing, the app list with artwork, `gateway-status` with the MAC address
  (the TV sends Wake-on-LAN itself), `start-session` and `stop-session`, `stop-host-session` and
  `switch-session`.
- The console toggle and Devices page entries.

`capabilities` lists only what we send: `hdrSupported:false` and no AV1. A request for anything else gets
`unsupported-settings`, and the host never falls back.

**Later in 3.x:**

- HEVC Main10 HDR (`profile-id=2`, levels 123/150/153/156; the colour-space extension offered but
  never sent).
- AV1, once its packetizer test passes against the TV.
- Host-side mouse mode on a long press of Start.
- Surround audio.
- Signalling over TLS, if the client gains it.

## Testing

- **Unit (core).**
  - Every JSON example in `protocol.md` parses and round-trips.
  - Unknown fields are ignored and a wrong `version` is rejected.
  - The HMAC proof has a fixed vector.
  - Nonces are single-use, and the comparison is constant time.
  - Pairing expiry and the single attempt work.
  - The connection limits (8 unauthenticated, 32 paired) hold.
  - `start-session` validation covers H.264 with HDR, 720p with HDR, 4K on H.264, a bitrate out of
    range and an unknown codec.
  - The gamepad mapping covers every button bit, trigger and stick sign.
- **Unit (host, str0m).**
  - The offer has one codec at a fixed PT and exactly one correct `imageattr`.
  - The offer has the playout-delay extension, and no RTX, transport-cc or REMB.
  - The H.265 fmtp level matches the mode.
  - Answer validation rejects a changed codec or profile.
  - Loopback between two in-memory str0m peers: an access unit goes through RTP and back
    byte-identical, with the marker bit on the last packet, timestamps from capture time, a PLI that
    sets `Session.idr`, and DataChannel messages in both directions.
- **GameStream unchanged.**
  - The existing suites pass with no edits to their expectations.
  - With the setting off, a test asserts that no WebRTC socket is open.
  - `git diff main -- rust/host/src/video_send* rust/core/src/packet*` stays empty.
- **Harness** (`rust/tests/webrtc_e2e.cjs`, Playwright like `console_browser.cjs`, our own code):
  - Setup:
    - Fetch the browser client at its pin into `rust/tests/fixtures/webrtc-web`.
    - Serve it with its own `npm start`.
    - Use installed Edge (`channel:'msedge'`) for H.264. For HEVC, try Chrome with
      `--enable-features=WebRtcAllowH265Receive`.
  - Run against an isolated test host: `--config-dir C:\tests\webrtc --port 48123 --bind 127.0.0.1`,
    `webrtc_port`/`webrtc_media_port` in the 48545+ range, `virtual_display_layout = extended`.
  - Run order:
    1. Pair: request, approve through `/api/pin`, then a wrong PIN and a cancel case.
    2. Authenticate, list the apps and fetch artwork.
    3. Start a stream, announce a gamepad and check that rumble comes back.
    4. Stream for 60 s.
    5. Stop the stream and the app.
  - Data collected:
    - `getStats` for video: framesDecoded/framesReceived, framesDropped, packetsLost, freezeCount and
      jitterBufferDelay per frame.
    - `getStats` for audio: packetsReceived and concealedSamples.
  - **Glass-to-glass:**
    - A clock page on the test host's virtual display paints a time code.
    - The client reads it back from each decoded frame with `requestVideoFrameCallback` and a canvas.
    - Both sides run on the same PC, so they share one clock.
  - Idle check before every run; nothing runs while the user is gaming.
- **"Tested." paragraph cites** (our numbers only), for H.264 1080p60 at 20 Mbps and HEVC 4K60 at
  50 Mbps (whatever Edge or Chrome can decode):
  - median and p95 glass-to-glass;
  - decoded/sent frame ratio;
  - packet loss;
  - host present-to-sent from the 5 s `stream timings` lines;
  - the media thread's CPU.

  It adds a GameStream loopback smoke with the setting off and on, which must not regress. The TV
  itself stays Experimental until a user reports on it.

## Briefs

Each brief gets its own worktree from `main` and owns only the files named. An implementer builds it
and a reviewer checks the diff before it merges.

- **B1. Protocol core (parallel with B2).**
  - Goal: `rust/core/src/webrtc/{mod,protocol,auth,capabilities,gamepad}.rs`, covering the serde
    message types for every protocol v2 message, nonce and HMAC auth, credential generation, the mode
    table and `start-session` validation, and the gamepad-state to `Input` mapping.
  - Constraints: no I/O, no new core dependencies, error codes exactly as in the protocol.
  - Acceptance: the core unit tests above pass under the fast core loop, and clippy and fmt are clean.
  - Report: files, test count, any protocol ambiguity.
- **B2. str0m media transport (parallel with B1).**
  - Goal: `rust/host/src/webrtc/{media,sdp}.rs`, plus the pinned str0m dependency in `rust/host/Cargo.toml`
    and `Cargo.lock`. That covers:
    - the Rtc builder with codecs, fixed PTs, extensions and fmtp;
    - offer post-processing and answer validation;
    - an ICE-lite single-socket media thread with a bounded frame channel;
    - the pacer;
    - Opus writes;
    - PLI/FIR to a callback;
    - DataChannel plumbing;
    - the NACK-without-RTX finding.
  - Constraints: no edits outside those files, the release profile builds on GNU, and nothing is wired
    into `stream.rs` yet.
  - Acceptance: the in-memory loopback tests pass. A bench reports the time from `submit` to the last
    packet for a 1 MB keyframe, and the SRTP cost per Mbps for `rust-crypto` against `wincrypto`. The
    full gate passes.
  - Report: test names, the bench numbers, the NACK finding.
- **B3. Signalling, discovery, pairing, console (after B1).**
  - Goal: `rust/host/src/webrtc/{server,discovery,clients}.rs`, the `webrtc_server`, `webrtc_port` and
    `webrtc_media_port` settings (console settings, `docs/configuration.md`), WebRTC pairing requests on
    the PIN page, and the badge, disable and unpair actions on the Devices page.
  - Server behaviour:
    - The WebSocket runs on its own axum listener.
    - The LAN filter and connection limits apply.
    - `gateway-status`, `capabilities`, `get-apps` and `get-app-artwork` come from the library.
    - `start-session` answers `unsupported-settings` until B4 lands.
  - Constraints: audit `Client.cert`, keep bind failures loud, and leave the GameStream paths untouched.
  - Acceptance:
    - A tokio WebSocket integration test covers pairing through `/api/pin`, auth success and failure,
      disabled and unpaired clients, the limits and the app list.
    - A discovery test covers private and public peers.
    - Console checks pass.
  - Report: files, tests, ports as shipped.
- **B4. Stream integration (after B2 and B3).**
  - Goal:
    - The `nvhttp::start` split.
    - `Launch.transport` and `Negotiated::webrtc`.
    - `webrtc::session` (start, stop, switch, stop-host).
    - The `send_frames` short-circuit.
    - The audio sink trait.
    - Gamepad input and rumble.
    - PLI to `Session.idr`.
    - Mutual exclusion with Moonlight.
  - Files: `rust/host/src/{nvhttp,stream}.rs`, `rust/core/src/{session,rtsp}.rs`,
    `rust/host/src/webrtc/session.rs`.
  - Constraints: the GameStream checks above hold. No edits to `video_send` or the core `packet`
    module. Every fix gets a regression test.
  - Acceptance: the full gate passes, plus a host test that starts a WebRTC session against a fake
    encoder and receives frames through a str0m receiver.
  - Report: diff summary, tests, anything that changed in a GameStream file.
- **B5. Harness, measurements, docs (after B4; the pairing and app-list legs can start after B3).**
  - Goal:
    - `rust/tests/webrtc_e2e.cjs` with the fixture fetch, getStats and the glass-to-glass clock.
    - The pacing A/B and the RTX A/B.
    - The 90/120 fps decision per mode.
    - `docs/webrtc.md` for users.
    - A row in `docs/hardware-matrix.md`.
    - The release-notes "Tested." figures.
  - Constraints: an isolated test host, idle checks, no runs while the user is gaming, and claims only
    from these runs. Revert any change that measures worse.
  - Acceptance: the harness passes three runs in a row for H.264 1080p60, and HEVC is at least
    established. Numbers go into `rust/PERFORMANCE.md`.
  - Report: the measurement table, the A/B outcomes, the modes offered.

## Risks

| Risk | Mitigation |
|---|---|
| The protocol moves (repos are weeks old) | Pins above; compare `protocol.md` before each brief and before the release |
| No HEVC decoder in headless Edge, and the Chrome flag may not work | Try Chrome with the flag. If it fails, HEVC is "established, frames received", and the TV ships as Experimental |
| Samsung Game Mode needs `imageattr` and may reject other offer details | Copy the fork's offer shape line for line, check our own offer in tests, and ship as Experimental |
| str0m 0.24 is pre-1.0 | Exact pin; the loopback and SDP tests catch changes on an upgrade |
| `rust-crypto` SRTP cost at 4K 100 Mbps | B2 bench; switch to `wincrypto` if it measures better |
| Port 8000 is taken | Loud bind error, configurable signalling port; discovery keeps 8000 |
| The `nvhttp::start` split regresses Moonlight launch | Behaviour-preserving split, nvhttp tests unchanged, reviewed before merge |
| Keyframe bursts overflow the socket at 4K | 4 MB send buffer plus the pacer A/B in B5 |
| Unencrypted signalling | Documented. Auth guards control, not confidentiality. LAN-only filter |
