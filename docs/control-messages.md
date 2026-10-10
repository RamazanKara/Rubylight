# Control messages

[Documentation](README.md) · [Configuration](configuration.md) · [Microphone](microphone.md)

Besides Moonlight's own control messages, Rubylight takes a few of its own from the client. Their wire formats are defined once, in the `rubylight-protocol` crate of [Rubylight Android](https://github.com/RamazanKara/rubylight-android) (`protocol/core`), which the host and the Android client both build. This page is for client authors.

## Negotiation

From 2.2.0 the host's RTSP `DESCRIBE` answer lists the Rubylight control messages it takes:

```
a=x-rl-control:0x5530,0x5531,0x5532
```

Send a message only when its id is in that list. Older hosts and other Moonlight hosts do not send the line; to them, send none of these messages. A host ignores control message ids it does not know.

## Framing

Each message travels on the control stream like Moonlight's client-to-host control messages: the 16-bit message type, then the payload, encrypted when the session encrypts its control stream. All fields are little-endian. Every payload starts with a version byte, currently `1`. A newer client may append fields; the host reads the fields it knows and ignores the rest. A payload that is too short, has another version or carries implausible values is ignored (logged at debug).

| Id | Name | Crate type | Host behaviour |
| --- | --- | --- | --- |
| `0x5530` | Phase lock report | `phase_lock::Report` | Times captures to the client's display latch while reports arrive about every 500 ms; after 3 s without one the host returns to its own pacing. |
| `0x5531` | Display luminance | `control::DisplayCaps` | Describes the client's screen in the HDR metadata ([below](#0x5531-display-luminance)). |
| `0x5532` | Reconfigure | `control::Reconfigure` | A request to change resolution or frame rate without reconnecting; see the crate for its format. |

## 0x5531 display luminance

Send it once the control stream is connected and again whenever the display changes, for example when a foldable switches screens or the window moves to another display. Sending the same values again is harmless; the host acts only on a change.

| Bytes | Field | Value |
| --- | --- | --- |
| 0 | version | `1` |
| 1 | flags | bit 0: the display can show HDR (HDR10 or HLG) right now |
| 2–3 | reserved | 0 |
| 4–7 | peak | peak luminance in hundredths of a nit (1000 nits = `100000`); 0 if unknown |
| 8–11 | average | maximum frame-average luminance in hundredths of a nit; 0 if unknown |
| 12–15 | black | minimum luminance in ten-thousandths of a nit (0.0005 nits = `5`); 0 if unknown |

The host ignores a message whose peak is outside 1–10,000 nits, whose average is above the peak, or whose black level is 1 nit or more.

What the host does with it:

- **In an HDR stream with a known peak**, the HDR metadata describes the client's screen: the peak becomes the mastering display maximum and MaxCLL, the frame-average MaxFALL and the full-frame luminance, and the black level the mastering display minimum. An unknown average or black level keeps the host display's value. The colour primaries and white point stay BT.2020 with D65, as before.
- **An HDR profile selected for the device on the host, or a peak brightness set for the device or app, takes precedence**: the host keeps describing its own display, because the user calibrated it for this client. SDR streams are unchanged.
- The host sends this metadata in Moonlight's HDR mode message (`0x010e`) and writes it into the video bitstream's HDR10 metadata. The first `0x010e` of a stream goes out once the encoder has the display's values, with the client's luminance if it has arrived by then. A later change sends `0x010e` once more, and only if the resulting values differ; the encoder is given the new values with its next metadata refresh, about once a second.
- The game itself renders for the host display. With a virtual display that display is created before the client connects, so its peak does not follow this message; select an HDR profile or peak brightness for the device on the host for that.
- The host log records `display caps` with the reported values and what was applied.

## Error correction reports

Moonlight's own FEC status message (`0x5502`, `SS_FRAME_FEC_STATUS` in moonlight-common-c) needs no announcement. A client that sends it for frames it had to repair or could not rebuild gets adaptive error correction from 2.2.0: the share of recovery packets rises with reported loss, returns to the configured `fec_percentage` when a frame could not be rebuilt, and falls slowly towards 5 % while no loss is reported. A client that never sends it keeps the configured percentage.
