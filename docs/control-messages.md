# Control messages

[Documentation](README.md) · [Configuration](configuration.md) · [Microphone](microphone.md)

Besides Moonlight's own control messages, Rubylight takes a few of its own from the client. Their wire formats are defined once, in the `rubylight-protocol` crate of [Nightjar](https://github.com/RamazanKara/nightjar) (`protocol/core`, `client/protocol/core` in this repo), which the host and Nightjar both build. This page is for client authors.

## Negotiation

From 2.2.0 the host's RTSP `DESCRIBE` answer lists the Rubylight control messages it takes:

```
a=x-rl-control:0x5530,0x5531,0x5532
```

Send a message only when its id is in that list. Older hosts and other Moonlight hosts do not send the line; to them, send none of these messages. A host ignores control message ids it does not know.

## PyroWave hints in the RTSP `ANNOUNCE`

A PyroWave client (record framing, `pyrowaveAdaptiveFec` or `pyrowaveFeatures` bit 0) may add these SDP attributes. A host that does not know a key ignores it, so a client sends them unconditionally.

| Attribute | Value | Host behaviour |
| --- | --- | --- |
| `x-ss-video[0].pyrowaveLinkMbps` | the client's wired link, Mbps | Caps the requested bitrate at 80% of the link (`network_pacing`). |
| `x-ss-video[0].pyrowavePaceMbps` | the pace the client's calibration found loss-free, Mbps | The sender's pacing rate when present (`network_pacing`). |
| `x-ss-video[0].pyrowaveRowOrder` | `1` | Sends the fine records of each 64-row picture band together, with what a band needs before the next band's, after the coarse records as always. Any other value, or no key, keeps the order by block index. A value that is not a number fails the `ANNOUNCE`, like the other numeric keys. |

Row order changes only the order of the records and the padding between packets (within 1% of the size on the test fixtures). Every record still carries its own block index, so a client that sorts or places records by index decodes either order. Under a lost tail, row order loses the last rows at every level and index order the finest level of the bottom rows; the coarse records and the critical FEC block are the same in both.

## Framing

Each message travels on the control stream like Moonlight's client-to-host control messages: the 16-bit message type, then the payload, encrypted when the session encrypts its control stream. All fields are little-endian. Every payload starts with a version byte, currently `1`. A newer client may append fields; the host reads the fields it knows and ignores the rest. A payload that is too short, has another version or carries implausible values is ignored (logged at debug).

| Id | Name | Crate type | Host behaviour |
| --- | --- | --- | --- |
| `0x5530` | Phase lock report | `phase_lock::Report` | Times captures to the client's display latch while reports arrive about every 500 ms; after 3 s without one the host returns to its own pacing. |
| `0x5531` | Display luminance | `control::DisplayCaps` | Records the client's screen capabilities for diagnostics ([below](#0x5531-display-luminance)). |
| `0x5532` | Reconfigure | `control::Reconfigure` | Changes the stream's resolution or frame rate without reconnecting ([below](#0x5532-reconfigure)). |
| `0x5536` | Host timing | `control::HostTimingRequest` | Sends reports of where each new picture's time went on the host, after its last packet left ([below](#0x5536-host-timing)). |
| `0x5537` | Clock sync | `control::ClockSyncRequest` | Answers at once with a `control::ClockSyncReply` on the video RTP clock ([below](#0x5537-clock-sync)). |
| `0x5538` | Link notice | `control::LinkNoticeRequest`, `control::LinkNotice` | Tells the client when it lowers the video bitrate for the network, and when it is back ([below](#0x5538-link-notice)). |

## 0x5536 host timing

A client asks for per-picture host timing and the host answers with reports; Moonlight's frame header holds only one host latency, written before the first packet, so it cannot hold the send.

The request is 4 bytes: version `1`, a flags byte (bit 0 turns reports on, `0` turns them off) and two reserved bytes. Send it once the stream starts. The host sends no reports to a client that did not ask, and keeps nothing for it.

The host answers on the same message type, unreliably. A report is a 4-byte header, version `1`, the record count, `extra` (see below) and a reserved byte, followed by `count` 20-byte records of five 32-bit words:

| Word | Field | Meaning |
| --- | --- | --- |
| 0 | `frame` | The frame number in the video packets (Moonlight's frame index). |
| 1 | `host_us` | The picture's present stamp to its last packet handed to the socket. |
| 2 | `queue_us` | Present to the encoder taking the picture: capture, the wait for the claim and the conversion. |
| 3 | `encode_us` | The encoder taking the picture to the encoded frame in the host's hands. |
| 4 | `send_us` | The first packet to the last one handed to the socket. |

`host_us = queue_us + encode_us + residual + send_us`; the residual (waiting for the sender and packetizing) is what the named stages leave. A record whose stages add up to more than `host_us` rejects the whole report. Only new pictures get a record, not static repeats.

### The extension

Header byte 2, `extra`, is the number of bytes per record in a trailing block: `0` in a v1 report, `16` from Rubylight 3.0. The version stays `1`. After the `count` records, `count × extra` bytes follow, one entry per record, in the same order:

| Offset | Field | Meaning |
| --- | --- | --- |
| 0 | `presented_at_us` (u64) | The capture's present stamp, in microseconds on the video RTP clock (the clock of [clock sync](#0x5537-clock-sync)). |
| 8 | `capture_us` (u32) | Present to the picture acquired. |
| 12 | `convert_us` (u32) | The host's claim of the picture to the encoder taking it. |

A reader takes the first 16 bytes of each entry and skips the rest, so later fields can be appended. The extension is valid only when `capture_us + convert_us <= queue_us`; a client drops the extension of a record that breaks this, or of a report that is cut short, and keeps the base records. With it, a client has every host stage on the host clock: capture present `P = presented_at_us`, acquired `P + capture_us`, submitted `P + queue_us`, encoded `P + queue_us + encode_us`, last packet sent `P + host_us`, first packet sent `P + host_us - send_us`. Hold and claim are `queue_us - capture_us - convert_us`. The game's own present is the frame's RTP timestamp.

An extended report carries at most 17 records (616 bytes). The limit is not the ENet packet size: Nightjar's patched moonlight-common-c hands a `0x5536` message to its Java layer only when it is at most 644 bytes (a v1 report of 32 records), and shipped Nightjar builds drop anything longer in native code. 17 extended records (`4 + 17 × 36`) fit and 18 do not. A v1 report carries up to 32. The Rust and Java decoders check only that the data holds `4 + count × 20` bytes and ignore the rest, so they read an extended report as a v1 one, but only if the transport delivers it. The host sends the extension for H.264, HEVC, AV1 and PyroWave alike.

## 0x5537 clock sync

A client probes the host's video clock, NTP style, to map frame timestamps to its own clock. Rubylight's VRR presentation (`present_clock` in rubylight-protocol) needs no shared clock to schedule frames; clock sync adds the host-present-to-client-present latency.

The request is 16 bytes: version `1`, three reserved bytes, a 32-bit sequence number and the client's send time as 64-bit microseconds of its own monotonic clock. The host answers each request with 32 bytes on the same message type, unreliably: version, three reserved bytes, the sequence, the client's time unchanged, then the times the host received the request and sent the reply. Host times are microseconds on the video stream's RTP clock: a frame stamped at `t` µs carries the RTP timestamp `t × 90 / 1000` modulo 2³². The host does not answer before the video stream starts. Four probes a second are plenty; the client keeps the exchanges with the shortest round trips (`present_clock::ClockOffset`).

## 0x5538 link notice

When the link-rate controller lowers the video bitrate because the network between the host and the device lost video, the picture gets softer and nothing else tells the player why. A client that asks hears about it and can say so in its overlay. Without the request the host sends nothing, so stock Moonlight clients never see the message.

The request (client to host) is 4 bytes: version `1`, a flags byte (bit 0 turns notices on, `0` off) and two reserved bytes. Send it once the stream starts, to a host that lists `0x5538` in `a=x-rl-control`. A client that asks while the host is already below the requested bitrate gets the standing notice at once.

The notice (host to client) is 12 bytes, sent reliably:

| Bytes | Field | Value |
| --- | --- | --- |
| 0 | version | `1` |
| 1 | state | `0` full (the requested bitrate), `1` limited (lowered, probing back up), `2` floor (at the lowest bitrate the host uses, and the network still loses video) |
| 2 | reason | `0` none (only with state `0`), `1` network |
| 3 | reserved | 0 |
| 4-7 | ceiling | the video bitrate the client asked for, as the encoder is given it, in kbps |
| 8-11 | current | the encoder bitrate now, in kbps |

The host sends one on each change of the controller's state or bitrate: a step down, a probe up, a failed probe, a floor, and the return to full. It sends at most one a second; when changes come faster, the newest follows when the second is up, and a return to full is never dropped. A change of state goes out when the second is up. A bitrate that only moves inside one state is calmed so the overlay does not flicker: it goes out after 5 seconds, or after the second when it moved by more than a tenth of the last one sent. A second enable request repeats the standing limit. It also sends the return to full when a reconfigure (`0x5532`) rebuilds the controller or the client takes over the bitrate itself (`/bitrate`). The first notice a client gets is never state `0`; it assumes full. A client clears its notice on state `0`. A payload that is shorter, has another version, state or reason, has a zero bitrate, or pairs state `0` with a reason or another state with none is ignored. The notice is off the video send path and adds no latency to it. `LinkNotice::decode` in rubylight-protocol parses it.

## 0x5531 display luminance

Send it once the control stream is connected and again whenever the display changes, for example when a foldable switches screens or the window moves to another display. Sending the same values again is harmless; the host logs only changes. Brightness changes on the same screen do not need another report.

| Bytes | Field | Value |
| --- | --- | --- |
| 0 | version | `1` |
| 1 | flags | bit 0: the display can show HDR (HDR10 or HLG) right now |
| 2–3 | reserved | 0 |
| 4–7 | peak | peak luminance in hundredths of a nit (1000 nits = `100000`); 0 if unknown |
| 8–11 | average | maximum frame-average luminance in hundredths of a nit; 0 if unknown |
| 12–15 | black | minimum luminance in ten-thousandths of a nit (0.0005 nits = `5`); 0 if unknown |

The host ignores a message whose peak is outside 1–10,000 nits, whose average is above the peak, or whose black level is 1 nit or more.

The host records the reported values for diagnostics. They do **not** replace HDR10 mastering luminance, MaxCLL, MaxFALL or black level, change the virtual display, or trigger an HDR mode update.

HDR10 metadata describes the captured source and the light levels of its pixels. Receiver capabilities are not content light levels. Earlier versions substituted the client's luminance without tone-mapping the pixels, which could change Android tone mapping and restart its decoder when brightness reports changed. The stream now keeps the source metadata; genuine source HDR changes still update the control message and encoded bitstream. An HDR profile or peak brightness selected on the host continues to configure the source display before capture.

## 0x5532 reconfigure

A client whose display changes during a stream, such as a foldable phone opening its inner screen or a window being resized, can ask for another size and rate. The stream continues on the same connection; there is no new RTSP exchange.

### The message

Send it on the encrypted control stream like any other control message (the keyframe request `0x0302`, for example), with type `0x5532` and this 12-byte payload, little-endian:

| Bytes | Field | Value |
| --- | --- | --- |
| 0 | version | `1` |
| 1 | reserved | `0` |
| 2–3 | width | pixels, even, 256–8192 |
| 4–5 | height | pixels, even, 256–8192 |
| 6–7 | reserved | `0` |
| 8–11 | frame rate | thousandths of a frame per second, 10 000–500 000 (120 fps is `120000`, 59.94 fps is `59940`) |

A payload that is shorter, has another version or has a value outside these limits is ignored. `Reconfigure::encode` in rubylight-protocol (`rp_reconfigure_encode` in C) writes it.

### What the host does

- **It waits for the client to settle.** The latest request is applied once no other has come for 200 ms, so a window being dragged or a hinge moving causes one switch, not many. Repeating the same request does not extend the wait. The host switches at most once a second; a request that comes sooner waits for that.
- **A request for the current size and rate changes nothing**, and withdraws a request still waiting.
- **The switch happens between frames.** The encoder is recreated at the new size and rate, and its first frame is a keyframe whose sequence header (SPS/PPS for H.264 and HEVC, the sequence header OBU for AV1) carries the new size. Frames already encoded at the old size can still arrive before it. Frame numbers continue without a gap.
- **Pacing follows the new rate.** A phase lock (`0x5530`) is released at a rate change and locks again from the client's next reports.
- **The PC's display keeps its mode.** The picture is scaled to the new size on the GPU, keeping its aspect ratio, with black bars where the shapes differ. Absolute mouse, touch and pen positions follow the new size. The display's refresh rate and a game's frame limit stay as they were set when the stream started, so a frame rate above the display's refresh repeats pictures.
- **Everything else stays as negotiated**: codec, HDR, chroma, bitrate and audio. To change the bitrate as well, use the client's usual bitrate request.

The host does not reply. It keeps the old size when:

- the stream uses PyroWave, whose sender and error correction are sized when the stream starts;
- `stream_reconfigure` is off for the host, the device or the app;
- the encoder cannot be created at the new size; it is then recreated at the old size, and the client receives a keyframe at the old size.

The host logs each switch as `reconfigure` with the old and new mode and how long the switch took, and each refusal as `reconfigure refused` with the reason.

### Client advice

- Send the message once the new display size is known, and round odd sizes to even ones.
- Keep decoding the old size until the keyframe at the new size arrives, then reconfigure the decoder from its sequence header. On Android, a decoder configured with the largest size the device can show (`KEY_MAX_WIDTH`, `KEY_MAX_HEIGHT`) and adaptive playback takes the new size without being recreated.
- Read the size of the picture from the stream, not from the request: the host may have kept the old one.
- Send it only to a host that supports it (Rubylight 2.2.0 and later). Other hosts ignore it, and the stream then keeps its size.

## Error correction reports

Moonlight's own FEC status message (`0x5502`, `SS_FRAME_FEC_STATUS` in moonlight-common-c) needs no announcement. A client that sends it for frames it had to repair or could not rebuild gets adaptive error correction from 2.2.0: the share of recovery packets rises with reported loss, returns to the configured `fec_percentage` when a frame could not be rebuilt, and falls slowly towards 5 % while no loss is reported. A client that never sends it keeps the configured percentage.

With the optional `host_adaptive_bitrate` setting, these validated reports also
activate host bitrate adaptation for conventional codecs. Reports are checked
against sent frame numbers and deduplicated before affecting either controller.
Standard `0x0200` periodic pings (8-byte payload beginning with little-endian 4)
provide feedback freshness. FEC report silence alone is not a clean-link signal:
upward probes also require sustained sent frames/bytes and stable ENet RTT.
The controller reserves audio, framing and FEC inside one wire budget.

`/api/abr/capabilities` retains `supported: false` for older client controllers
and adds `host_adaptive_bitrate`, `client_bitrate_takes_ownership: true` and the
`host_loss_adaptation` feature. A successful authenticated `/bitrate` request
atomically disables host bitrate and adaptive pacing control for that client's
current sessions. A client should send its current encoder bitrate once when
starting its own controller; no lease renewal is needed. New sessions reset
ownership. PyroWave is excluded from this host controller.

By default (`link_rate_adaptation`, [configuration](configuration.md#lower-bitrate-when-the-link-cannot-carry-it)) the host also lowers the bitrate when the network cannot carry the stream. It reads the same validated `0x5502` reports (frames the client could not rebuild, and the share of packets reported missing), and the same `0x0200` pings for freshness. The keyframe request `0x0302` and reference invalidation count as network evidence only from a client that sends no `0x5502` at all, and only when the host or the client's decode queue did not cause them. A `/bitrate` request from the client turns it off for that session, as it does for the controller above.
