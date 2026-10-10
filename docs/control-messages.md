# Control messages

[Docs](README.md) · [Configuration](configuration.md) · [Microphone](microphone.md)

Rubylight understands a few control-stream messages beyond the ones Moonlight defines. Rubylight Android sends them; other clients can too. Their wire formats live in [rubylight-protocol](https://github.com/RamazanKara/rubylight-android/tree/main/protocol), which a client can use directly (Rust, or C through its FFI header).

| Type | Direction | Message |
| --- | --- | --- |
| `0x5530` | client to host | Phase lock report: the client's display timing, so captures reach it just before its refresh. |
| `0x5532` | client to host | Change the stream's resolution or frame rate without reconnecting. |

A host that does not know a message ignores it, and every message starts with a version byte; a newer client may append fields, which an older host ignores.

## For client developers: changing resolution or frame rate (0x5532)

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
