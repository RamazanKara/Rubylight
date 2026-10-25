# WebRTC server design

Plan of record for issue #50: an optional WebRTC transport beside GameStream for the Moonlight WebRTC TV
protocol (Samsung Tizen TV app and the browser client). Target: Rubylight 3.0.0. Status lives in the
3.0 plan's status table.

## Pinned references

| What | Pin | Licence | Use |
|---|---|---|---|
| Protocol and TV app, [mlopezsegura/Monlight-Tizen-WebRtc-Client](https://github.com/mlopezsegura/Monlight-Tizen-WebRtc-Client) | `193ef539c3cf` (2026-10-19); `docs/protocol.md` last changed `fdc5f9e98162` (2026-10-24: the client's `request-keyframe` on the control channel and `client-log`, both handled), protocol version 2; checked again 2026-10-25 against `cc1e8dae0c60`, no protocol change | GPL-3.0 | The contract we implement |
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
6. **Everything protocol v2 defines, plus two extensions.** H.264, HEVC and AV1, HDR (HEVC Main10 and
   AV1 Main 10-bit) and 30 to 120 fps are part of protocol v2 and are in scope as written there.
   Surround audio and keyboard and mouse input are not in protocol v2 (scope widened on 2026-10-24), so
   they are Rubylight extensions:
   - advertised in a `capabilities.extensions` object, which stock clients ignore;
   - used only when the client asks for them (`audio.channels` 6 or 8, a client-opened `input`
     DataChannel);
   - `version` stays 2, and a stock TV or browser client sees exactly the protocol v2 behaviour.

   The TV app's own mouse path, a long press of Start that turns the gamepad into a mouse, works on
   the host side as the reference host does it, so TVs get a mouse without the extension.

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
| HDR and AV1 | H.265 profile-tier-level handling plus custom fmtp; `Codec::H265` and `Codec::Av1` exist but are `#[doc(hidden)]` | fmtp strings | fmtp strings |
| Surround | No multiopus codec: the client's answer is edited (below) | No multiopus codec | No multiopus codec |

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
- HDR: set the HEVC Main10 fmtp (`profile-id=2`) or the AV1 fmtp (`profile=0`). The level is the
  lowest one, never below 4.1 as in the reference host, whose picture size, sample rate and Main-tier
  bitrate hold the stream: 4.1 at 1080p60, 5.0 at 1440p60 or 1080p120, 5.1 at 4K60 or 1440p120, 5.2
  at 4K120 at moderate bitrates. A bitrate no Main-tier level holds moves to High tier. The same rule
  sets the SDR HEVC and AV1 levels. Offer the colour-space
  extension `http://www.webrtc.org/experiments/rtp-hdrext/color-space` and never send it, as the
  protocol requires.
- AV1: str0m's `Av1` packetizer is hidden API. B2 proves it with a loopback test: a temporal unit from
  AMF goes out as RTP and comes back as the same OBUs, minus the temporal delimiter.
- Surround: str0m knows only `opus/48000/2`, and Chromium refuses `multiopus` in a remote offer, so
  a client edits only its own answer:
  - offer: always standard stereo Opus, `a=rtpmap:<pt> opus/48000/2` with the protocol's fmtp
    (`minptime=20;maxaveragebitrate=128000;stereo=1;sprop-stereo=1;useinbandfec=0`), whatever
    `audio.channels` asked for. The offers for 2, 6 and 8 channels are identical;
  - answer for 2 channels: `opus/48000/2`;
  - answer for 6 or 8 channels: `a=rtpmap:<pt> multiopus/48000/<6|8>` at the offered audio PT with
    `a=fmtp:<pt> channel_mapping=<m>;num_streams=<s>;coupled_streams=<c>;...`. libwebrtc's own layouts
    are 5.1 `0,4,1,2,3,5`, 4 streams, 2 coupled, and 7.1 `0,6,1,2,3,4,5,7`, 5 streams, 3 coupled. The
    host takes the stream count, coupled count and mapping when libopus can create an encoder with
    them: at least one stream, no more coupled streams than streams, every mapping entry below
    `streams + coupled` (or 255, a silent channel), and every stream's channels mapped, both of a
    coupled one. Any such order is taken, canonical or not, and so is a duplicate entry. It rewrites the line back to
    `opus/48000/2` with the protocol fmtp before str0m parses it, and configures the Opus encoder with
    exactly that layout (`OpusLayout` with a custom layout). Unknown or extra fmtp keys are ignored;
  - a stereo answer to a surround request, a changed channel count or an invalid layout is an error.
    The host never falls back to stereo.

  str0m's Opus packetizer passes a payload through untouched, so a multistream packet goes out as is.
  Only Chromium-based receivers decode `multiopus`. A client that answers without it gets an error,
  never a silent downmix.

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
                                         │  input DataChannel ──▶ Input::Keyboard / Relative / Absolute /
                                         │                        MouseButton / Scroll (extension)
                                         ◀──  rumble, mouse-mode ── control DataChannel
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
  `Negotiated::webrtc(width, height, fps, codec, hdr, bitrate, channels)`, builds it from
  `start-session` instead, with 2, 6 or 8 audio channels, 5 ms Opus packets, 1 slice, no FEC and no
  reference-frame invalidation. `Media::start` then runs unchanged. The WebRTC transport doesn't start the GameStream video, audio
  or ENet control sockets. The video peer that GameStream learns from Moonlight's UDP ping comes from
  the address ICE selects instead.
- **Video.** `webrtc_sender` is `None` unless the session is WebRTC. It hands each `Encoded` frame
  (Annex-B access unit) to the media thread over a bounded channel and returns.
  - The RTP timestamp is the 90 kHz value of the frame's capture instant, measured from session start
    and monotonic.
  - Each frame also gets a playout-delay of min=max=0.
  - A frame that misses its deadline is dropped and followed by an IDR request, the way GameStream
    recovery handles it. No queue builds up.
- **HDR.** A `start-session` with `hdr:true` uses the GameStream HDR path unchanged:
  - The host display must already be in HDR. If it isn't, or leaves HDR during the stream, the session
    ends with an error, as the protocol says. The host never tone-maps to SDR.
  - The encoder writes HEVC Main10 or AV1 Main 10-bit with BT.2020 primaries, PQ transfer and limited
    range. The colour description sits in the HEVC VUI and the AV1 sequence header.
  - Mastering-display and content-light metadata come from the display's `Session.hdr_metadata`
    through the existing AMF metadata path (HEVC SEI, AV1 metadata OBUs), refreshed when the display
    changes it. GameStream sends the same values over ENet; WebRTC has only the bitstream.
- **AV1.** The AMF encoder probe decides. `capabilities` lists AV1 only when the probe found an AV1
  encoder, and lists it in `hdrCodecs` only when that encoder also does Main 10-bit. A request for
  AV1 on a GPU without it gets `unsupported-settings`. The capabilities wording never names GPU
  generations. AMF hands over a temporal unit per frame; the temporal delimiter is dropped before
  packetizing, as the AV1 RTP spec requires.
- **Audio.** `audio()` keeps its WASAPI capture and Opus encoder. The packet sink becomes a small
  trait: GameStream RTP/FEC (today's code, unchanged) or the WebRTC channel. Packets are 5 ms (240
  samples) at 48 kHz, in their own media stream with a separate msid so the browser doesn't hold video
  back for lip sync.
  - Stereo uses the protocol's fmtp
    (`minptime=20;maxaveragebitrate=128000;stereo=1;sprop-stereo=1;useinbandfec=0`) unless B2 shows a
    receiver rejecting 5 ms packets under it.
  - 5.1 and 7.1 (extension) use Opus multistream with the layout the answer negotiated and the
    bitrate GameStream's surround uses for that stream count, sent as `multiopus` (see Library).
    Capture follows the GameStream surround path, including its upmix and channel order. The encoder
    and the receiver use the same mapping, so each capture channel arrives on its own output channel.
  - Client notes: a surround client munges its own answer to `multiopus/48000/<N>` with the canonical
    layout above. Any Chromium client should put `stereo=1` in its own answer's Opus fmtp, or it
    decodes mono: the host offer already carries `stereo=1`, but the receiver decides from its own
    description.
  - The video budget subtracts the audio bitrate of the chosen layout.
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
  - **Gamepad mouse mode** (what the TV app offers today). Holding Start for 750 ms and releasing it
    toggles mouse mode for that controller, as in the reference host:
    - The stick moves the pointer every 50 ms on a cubic curve with a small deadzone, and each D-pad
      press scrolls one notch (up and down vertical, left and right horizontal).
    - A, B, X, LB and RB act as left, right, middle, back and forward buttons. Buttons held when the
      mode turns on stay suppressed until released.
    - The host reports `{"v":2,"type":"mouse-mode","controllerId":..,"controllerSlot":..,"active":..}`
      on the control channel.
    - Leaving mouse mode, a disconnect or the end of the stream releases every simulated button.
    - B1 copies the field names, constants and button choice from the reference's `input_bridge` and
      cites it; the TV app's gamepad tests show what it expects.
  - **Keyboard and mouse** (extension). The client opens a DataChannel labelled `input` (ordered,
    reliable). The host accepts it only after authentication and closes any other unknown label.
    Messages are JSON with `"v":1`:
    - `{"type":"key","code":"KeyA","down":true}`: `code` is the DOM `KeyboardEvent.code`. A core table
      maps it to the Windows virtual key that `Input::Keyboard` carries. Modifiers are ordinary keys.
      An unknown code is dropped and counted in the stream summary.
    - `{"type":"text","text":"é"}`: `Input::Text`, at most 32 characters per message, for IME output
      and characters with no key.
    - `{"type":"mouse-move","dx":-3,"dy":5}`: relative motion (pointer lock), clamped to i16, becomes
      `Input::Relative`.
    - `{"type":"mouse-position","x":0.5,"y":0.25}`: absolute position as a fraction of the video,
      clamped to 0..1, becomes `Input::Absolute` against the stream's width and height.
    - `{"type":"mouse-button","button":0,"down":true}`: DOM button numbers 0 to 4 (left, middle, right,
      back, forward) become Moonlight buttons 1, 2, 3, 4 and 5.
    - `{"type":"wheel","dx":0,"dy":120}`: 120 units per notch, DOM signs (positive `dy` scrolls down),
      becomes `Input::Scroll` with the sign Moonlight uses (up is positive).

    The client's `perm` bits gate keyboard and mouse exactly as for a Moonlight client. When the
    channel closes, the stream stops or the session switches, the host releases every key and button
    it holds for that client, so nothing stays stuck. The cursor comes from capture, as it does for
    Moonlight.
  - `capabilities.extensions` advertises
    `{"audioChannels":[2,6,8],"input":{"label":"input","version":1,"keyboard":true,"mouse":true}}`.
    Surround is negotiated in the answer (see Library), so the shape is unchanged; a client that asks
    for 6 or 8 channels must answer with `multiopus`.
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
- **Bitrate.** Video gets the requested bitrate minus the audio layout's bitrate (128 kbps for stereo)
  and the RTP/SRTP overhead. Valid requests are 10,000 to 100,000 kbps.
- **Input.** Keyboard, mouse and gamepad messages go from the media thread to a dedicated input thread
  over a channel, with no batching, so a slow `SendInput` can never stall the media thread's pacing.
  That is the same arrangement as ENet's control thread, which injects on its own thread. Mouse motion
  is not coalesced on the host. The queue delay, from the transport receiving a frame to the worker
  calling the injector, is recorded per message and its maximum and 99th percentile are logged in the
  stream's "WebRTC input summary" (`queue_max_us`, `queue_p99_us`) next to the unknown-code count, so B5
  can report the cost of the hop. The summary also splits it (`wait_*`, the channel; `refresh_*`, the
  injector's periodic refresh) and times the injector call (`apply_*`), which `SendInput` holds for as
  long as the desktop's low-level hooks take; on the laptop's echo window a mouse click holds it for
  4 to 28 ms, and the log names the events of any call over 2 ms.
- **Recovery.**
  - PLI, FIR and the control channel's `request-keyframe` set `Session.idr` and wake recovery, the same
    as a Moonlight IDR request.
  - There is no RTX in the MVP. B2 finds out whether str0m 0.24.1 can answer NACK without RTX. B5
    measures a 0.1 % loss leg with RTX on and off, and the better one ships.
- **No FEC and no reference-frame invalidation.** WebRTC has neither. Intra refresh stays off, as on
  GameStream.

## Scope

**MVP (3.0.0), labelled Experimental:**

- The protocol's video modes: H.264 at 720p, 1080p and 1440p; HEVC and AV1 at 720p, 1080p, 1440p and
  4K, all in SDR.
- HDR: HEVC Main10 and AV1 Main 10-bit at 1080p, 1440p and 4K, with BT.2020 PQ colour description and
  mastering-display and content-light metadata in the bitstream.
- AV1 wherever the AMF probe finds an AV1 encoder.
- 30 and 60 fps. 90 and 120 fps are offered only for modes where B5 measures them clean.
- Opus stereo audio, and 5.1 and 7.1 Opus multistream (extension).
- Gamepads with rumble, and gamepad mouse mode on a long press of Start.
- Keyboard and mouse over the `input` DataChannel (extension).
- Discovery, signalling, PIN pairing, the app list with artwork, `gateway-status` with the MAC address
  (the TV sends Wake-on-LAN itself), `start-session` and `stop-session`, `stop-host-session` and
  `switch-session`.
- The console toggle and Devices page entries.

`capabilities` lists only what this host can send: AV1 and AV1 HDR follow the encoder probe, and HDR
modes are listed with `hdrExperimental:true`. A request for anything else gets `unsupported-settings`,
and the host never falls back: no SDR for HDR, no H.264 for AV1, no stereo for surround.

Everything ships as Experimental. Where B5 cannot test a part on our own devices (the Samsung TV
itself, HDR decode inside a browser), it is hardened and fake-tested, and stays labelled Experimental.

**Later in 3.x:**

- Signalling over TLS, if the client gains it.
- Touch, pen and controller motion over the `input` channel.
- Moving the extensions into the upstream protocol, if its author wants them.

## Testing

- **Unit (core).**
  - Every JSON example in `protocol.md` parses and round-trips.
  - Unknown fields are ignored and a wrong `version` is rejected.
  - The HMAC proof has a fixed vector.
  - Nonces are single-use, and the comparison is constant time.
  - Pairing expiry and the single attempt work.
  - The connection limits (8 unauthenticated, 32 paired) hold.
  - `start-session` validation covers H.264 with HDR, 720p with HDR, 4K on H.264, a bitrate out of
    range, an unknown codec, AV1 and AV1 HDR with and without the encoder, and `audio.channels` 2, 6
    and 8 against 1, 4 and 10.
  - `capabilities` matches the protocol's mode table for each encoder probe result, and carries the
    `extensions` object.
  - The gamepad mapping covers every button bit, trigger and stick sign.
  - Gamepad mouse mode: the 750 ms toggle, motion curve and deadzone, button mapping and suppression,
    the `mouse-mode` message, and release of every simulated button on exit and disconnect.
  - Keyboard and mouse: every `KeyboardEvent.code` in the table maps to its virtual key, an unknown
    code is dropped, text length is capped, button numbers, wheel signs, absolute clamping and
    scaling, and the held-input tracker releases everything on close.
- **Unit (host, str0m).**
  - The offer has one codec at a fixed PT and exactly one correct `imageattr`.
  - The offer has the playout-delay extension, and no RTX, transport-cc or REMB.
  - The H.265 fmtp level matches the mode, for Main and Main10.
  - The AV1 fmtp matches the mode, and HDR offers carry the colour-space extension, which is never
    sent.
  - Surround negotiation: the offer is the same standard stereo Opus for 2, 6 and 8 channels, a
    Chrome-shaped munged answer for 6 and 8 yields its layout for the encoder, str0m sees
    `opus/48000/2`, and a stereo answer to a surround request, a changed channel count or an invalid
    layout is rejected.
  - Answer validation rejects a changed codec or profile.
  - Loopback between two in-memory str0m peers: an H.264, HEVC and AV1 access unit goes through RTP
    and back byte-identical (AV1 without its temporal delimiter), with the marker bit on the last
    packet, timestamps from capture time, a PLI that sets `Session.idr`, a multistream Opus packet
    unchanged, and DataChannel messages in both directions, including a client-opened `input`
    channel.
- **Unit (host, stream).**
  - A WebRTC HDR session against a fake encoder: the encoder config asks for 10-bit BT.2020 PQ, and
    the session ends with an error when the display is SDR or leaves HDR.
  - The bitstream of a real AMF HDR encode (an AMF test, skipped without AMF) has the expected HEVC
    VUI and SEI, or AV1 sequence header and metadata OBUs.
  - Keyboard and mouse messages reach the input dispatcher, `perm` bits block them, and a closed
    channel releases held keys.
- **GameStream unchanged.**
  - The existing suites pass with no edits to their expectations.
  - With the setting off, a test asserts that no WebRTC socket is open.
  - `git diff main -- rust/host/src/video_send* rust/core/src/packet*` stays empty.
- **Harness** (`rust/tests/webrtc_e2e.cjs`, Playwright like `console_browser.cjs`, our own code):
  - Setup:
    - Fetch the browser client at its pin into `rust/tests/fixtures/webrtc-web`.
    - Serve it with its own `npm start`.
    - Use installed Edge (`channel:'msedge'`) for H.264 and AV1. For HEVC, try Chrome with
      `--enable-features=WebRtcAllowH265Receive`.
    - The browser client asks for none of the extensions, so a small protocol v2 client of our own
      (`rust/tests/webrtc_client/`, plain JS in the same Playwright browser) drives the HDR, AV1,
      surround and keyboard and mouse legs.
  - Run against an isolated test host: `--config-dir C:\tests\webrtc --port 48123 --bind 127.0.0.1`,
    `webrtc_port`/`webrtc_media_port` in the 48545+ range, `virtual_display_layout = extended`.
  - Run order:
    1. Pair: request, approve through `/api/pin`, then a wrong PIN and a cancel case.
    2. Authenticate, list the apps and fetch artwork.
    3. Start a stream, announce a gamepad and check that rumble comes back. Toggle mouse mode and
       check the `mouse-mode` message and the pointer movement on the host.
    4. Stream for 60 s.
    5. Stop the stream and the app.
    6. With our own client:
       - AV1 1080p60 SDR for 60 s.
       - HDR: HEVC Main10 and AV1 10-bit at 4K60 with the test host's virtual display in HDR. The
         host checks the bitstream (10-bit, BT.2020, PQ, metadata present). The client checks that
         frames decode, and `getStats` reports the codec it should. A real HDR picture in a browser
         is not something we can verify, so HDR stays Experimental.
       - 5.1 and 7.1: the test host plays one tone per channel, and the client checks each channel
         with a Web Audio `ChannelSplitterNode` and an `AnalyserNode`, so a wrong mapping fails.
       - Keyboard and mouse: an echo window on the test host's virtual display logs what arrives. The
         client sends a fixed script of keys, text, relative and absolute motion, buttons and wheel
         notches, and the harness compares the logs. Then it closes the channel with a key held and
         checks that the key is released.
       - Injected input reaches the real desktop session, so the keyboard and mouse leg runs only on
         the 780M laptop or when this PC is idle and nobody is using it.
  - Data collected:
    - `getStats` for video: framesDecoded/framesReceived, framesDropped, packetsLost, freezeCount and
      jitterBufferDelay per frame.
    - `getStats` for audio: packetsReceived and concealedSamples.
  - **Glass-to-glass:**
    - A clock page on the test host's virtual display paints a time code.
    - The client reads it back from each decoded frame with `requestVideoFrameCallback` and a canvas.
    - Both sides run on the same PC, so they share one clock.
  - Idle check before every run; nothing runs while the user is gaming.
- **"Tested." paragraph cites** (our numbers only), for H.264 1080p60 at 20 Mbps, AV1 1080p60 at
  20 Mbps and HEVC 4K60 at 50 Mbps (whatever Edge or Chrome can decode), plus pass or fail for the HDR,
  surround and keyboard and mouse legs:
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

- **B1. Protocol core (parallel with B2), in two parts that run side by side.**
  - **B1a, protocol.** Goal: `rust/core/src/webrtc/{mod,protocol,auth,capabilities}.rs`, covering the
    serde message types for every protocol v2 message, nonce and HMAC auth, credential generation, the
    mode table with HDR and AV1 driven by an encoder-probe input, the `extensions` object, and
    `start-session` validation including HDR, AV1 and `audio.channels`.
  - **B1b, input.** Goal: `rust/core/src/webrtc/{gamepad,input}.rs`, covering the gamepad-state to
    `Input` mapping, gamepad mouse mode as a pure state machine driven by timestamps, the `input`
    channel messages, the `KeyboardEvent.code` to virtual-key table, and a held-input tracker that
    yields the releases on close. It adds one `mod` line each to B1a's `mod.rs`.
  - Constraints: no I/O, no new core dependencies, error codes exactly as in the protocol, and the
    extension messages exactly as in this doc.
  - Acceptance: the core unit tests above pass under the fast core loop, and clippy and fmt are clean.
  - Report: files, test count, any protocol ambiguity.
- **B2. str0m media transport (parallel with B1).**
  - Goal: `rust/host/src/webrtc/{media,sdp}.rs`, plus the pinned str0m dependency in `rust/host/Cargo.toml`
    and `Cargo.lock`. That covers:
    - the Rtc builder with codecs (H.264, HEVC Main and Main10, AV1), fixed PTs, extensions and fmtp;
    - offer post-processing (`imageattr`, the colour-space extension for HDR)
      and answer validation (codec, profile, the multiopus answer's channels and mapping, rewritten to
      stereo for str0m);
    - an ICE-lite single-socket media thread with a bounded frame channel;
    - the pacer;
    - Opus writes, stereo and multistream;
    - PLI/FIR to a callback;
    - DataChannel plumbing, including accepting the client-opened `input` channel after
      authentication and closing unknown labels;
    - the NACK-without-RTX finding.
  - Constraints: no edits outside those files, the release profile builds on GNU, and nothing is wired
    into `stream.rs` yet.
  - Acceptance: the in-memory loopback tests pass for H.264, HEVC, AV1, stereo and multistream Opus
    and the `input` channel. A bench reports the time from `submit` to the last packet for a 1 MB
    keyframe, and the SRTP cost per Mbps for `rust-crypto` against `wincrypto`. The full gate passes.
  - Report: test names, the bench numbers, the NACK finding, and whether str0m's AV1 packetizer needed
    anything beyond dropping the temporal delimiter.
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
    - `Launch.transport` and `Negotiated::webrtc` with HDR and the channel count.
    - `webrtc::session` (start, stop, switch, stop-host).
    - The `send_frames` short-circuit.
    - HDR: the display check at start and during the stream, the 10-bit encoder config, and the
      metadata path into the bitstream.
    - AV1: the encoder probe feeding `capabilities`, and dropping the temporal delimiter.
    - The audio sink trait, with stereo and surround.
    - PLI to `Session.idr`.
    - Mutual exclusion with Moonlight.
  - Files: `rust/host/src/{nvhttp,stream}.rs`, `rust/core/src/{session,rtsp}.rs`,
    `rust/host/src/webrtc/session.rs`, and in `rust/windows` only what the AV1 HDR metadata needs if
    GameStream doesn't already do it.
  - Constraints: the GameStream checks above hold. No edits to `video_send` or the core `packet`
    module. Every fix gets a regression test.
  - Acceptance: the full gate passes, plus host tests that start WebRTC sessions (H.264, HEVC HDR,
    AV1, 5.1) against a fake encoder and receive frames and audio through a str0m receiver.
  - Report: diff summary, tests, anything that changed in a GameStream file.
- **B4i. Input integration (after B4).**
  - Goal: `rust/host/src/webrtc/input.rs` and its hooks in `session.rs`: gamepad input and rumble,
    gamepad mouse mode with its 50 ms motion tick and `mouse-mode` messages, keyboard and mouse from
    the `input` channel into the dispatcher, `perm` checks, and the held-input release on close, stop
    and switch.
  - Constraints: the same dispatcher and gamepad worker as the ENet path, with no changes to their
    GameStream behaviour.
  - Acceptance: host tests for each path, including a stuck-key test, and the full gate.
  - Report: diff summary, tests.
- **B5. Harness, measurements, docs (after B4i; the pairing and app-list legs can start after B3).**
  - Goal:
    - `rust/tests/webrtc_e2e.cjs` with the fixture fetch, getStats and the glass-to-glass clock.
    - `rust/tests/webrtc_client/`, our own small client for the HDR, AV1, surround and keyboard and
      mouse legs, plus the host-side tone player and input echo window.
    - The pacing A/B and the RTX A/B.
    - The 90/120 fps decision per mode.
    - `docs/webrtc.md` for users, including the extensions so client authors can use them.
    - A row in `docs/hardware-matrix.md`.
    - The release-notes "Tested." figures.
  - Constraints: an isolated test host, idle checks, no runs while the user is gaming, the keyboard and
    mouse leg only on the laptop or an idle, unattended PC, and claims only from these runs. Revert any
    change that measures worse.
  - Acceptance: the harness passes three runs in a row for H.264 1080p60 and AV1 1080p60, HEVC is at
    least established, and the HDR, 5.1, 7.1 and keyboard and mouse legs pass. Numbers go into
    `rust/PERFORMANCE.md`.
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
| str0m's H.265 and AV1 support is hidden API | Exact pin; loopback tests for both; B2 reports anything it had to work around |
| Only Chromium decodes `multiopus`; the TV may not | Surround is an extension a client asks for. An answer without it is an error, never a silent downmix |
| A browser can't show whether HDR looks right | Host-side bitstream checks plus decoded frames; HDR ships Experimental |
| Injected keyboard and mouse reach the real desktop | `perm` bits, auth and the LAN filter; release of held input on every close; the test leg runs only on the laptop or an idle PC |
| The extensions clash with a later protocol version | Advertised under `extensions`, opt-in per session, `version` stays 2; offered upstream later |
