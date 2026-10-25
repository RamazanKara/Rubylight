# WebRTC server (Experimental)

[Documentation](README.md) · [Configuration](configuration.md) · [Features](features.md) · [Hardware matrix](hardware-matrix.md)

Rubylight can stream over WebRTC to a Samsung Tizen TV or to a browser, next to the usual Moonlight stream. It is off by default and Experimental. It speaks the Moonlight WebRTC TV protocol, version 2, which [mlopezsegura](https://github.com/mlopezsegura) designed for the [Tizen TV app](https://github.com/mlopezsegura/Monlight-Tizen-WebRtc-Client) and the [browser client](https://github.com/mlopezsegura/moonlight-webrtc-web). The Moonlight path does not change when this is off, and no WebRTC socket is open.

The first part is for people who stream. The second part is for people who write a client.

## For users

### Turn it on

1. Open the console, then **Settings**, and find the **WebRTC** group under Network.
2. Switch on **WebRTC server (Experimental)**.
3. Save, and restart Rubylight. The setting needs a restart.

If another program already holds a port, the console shows a `webrtc_server` warning, the log names the error and the server does not run. Rubylight never moves to another port on its own. Pick a free **WebRTC signalling port** instead.

### Ports and firewall

| What | Port | Setting |
| --- | --- | --- |
| Discovery | UDP 8000, always | none |
| Signalling (WebSocket) | TCP 8000 | `webrtc_port` |
| Media, audio and input | UDP 8001 | `webrtc_media_port` |

TVs send their discovery broadcast to UDP 8000, so discovery stays there even when you move the signalling port. The reply tells the TV which signalling port to use. The installer's Windows Firewall rule allows `butterpollo.exe` inbound on every port, so an installed host needs nothing more. On a portable host, allow these three ports. The full list of settings is in [Configuration](configuration.md#webrtc-server-experimental).

Broadcasts do not cross routers. A TV on another subnet needs the PC's address entered by hand.

### Pair a TV or a browser

1. Open the app on the TV or the browser client. It finds the PC, or you enter its address.
2. The app shows a four-digit PIN.
3. In the console, open the **PIN** page. The request appears next to Moonlight's, with the device name.
4. Enter the PIN there and confirm.

The app then holds a client ID and a secret, and signs in on its own from then on. The secret is stored in `sunshine_state.json` and is never shown in the console. A request lasts 5 minutes and allows one attempt.

### The Devices page

Paired TVs and browsers are listed on **Devices** with a **WebRTC** badge. Disable a device to keep it paired but refuse it. Unpair it to remove it. Either one disconnects the device at once, and ends its stream. At most 32 are kept; the oldest go first.

### What streams

| Part | What you get |
| --- | --- |
| Video | H.264, HEVC and AV1 at 720p, 1080p and 1440p; HEVC and AV1 also at 4K. AV1 appears only when the GPU's AMD encoder supports it. 30 and 60 fps. |
| HDR | HEVC Main10 and AV1 10-bit at 1080p, 1440p and 4K. The host display must already be in HDR. If it is not, or it leaves HDR during the stream, the stream ends with an error. The host never tone-maps to SDR. |
| Audio | Stereo, 5.1 and 7.1. Surround needs a client that asks for it and decodes it, which today means a Chromium-based browser. |
| Gamepad | Gamepads with rumble. |
| Gamepad as a mouse | Hold Start for 750 ms and release it. The stick moves the pointer, the D-pad scrolls, and A, B, X, LB and RB click. Do it again to turn it off. |
| Keyboard and mouse | From a client that opens the `input` channel (see below). The browser client does not. |

A request for anything the host cannot send is refused with an error. There is no fallback: no SDR for an HDR request, no H.264 for AV1, no stereo for a surround request.

One stream runs at a time. A WebRTC stream and a Moonlight stream exclude each other, and a start that conflicts gets an error instead of taking over.

### Network and safety

Signalling is plain `ws://`, so it is not encrypted. Signing in keeps other devices from using or interrupting the stream; it does not hide what the app and the PC say to each other. The media itself is encrypted (DTLS-SRTP). Use it on a trusted LAN only.

Rubylight serves only peers on the local network: loopback, private (RFC 1918), link-local, unique-local IPv6 and carrier-grade NAT (100.64.0.0/10) addresses. Anything else is closed before its first message and logged. Do not forward these ports on your router.

## For client authors

Rubylight implements protocol version 2 as the upstream [protocol document](https://github.com/mlopezsegura/Monlight-Tizen-WebRtc-Client/blob/main/docs/protocol.md) describes it: discovery, pairing, authentication, apps, `start-session`, `switch-session`, the stop messages and the host's offer. Read that for the messages. This part covers what Rubylight does that you need to know, and its two extensions. The reference host is [Sunshine-Web-RTC](https://github.com/mlopezsegura/Sunshine-Web-RTC). A stock client sees exactly protocol v2.

### The host's offer

The host sends the offer. It carries one video codec at a fixed payload type, one Opus audio track, and two DataChannels it creates itself:

| Label | Kind | Use |
| --- | --- | --- |
| `control` | ordered, reliable | Host to client: `rumble` and `mouse-mode`. Client to host: `request-keyframe`. |
| `gamepad` | unordered, no retransmits | Client to host: gamepad messages. |

The host is ICE-lite. Its only candidate is its own address plus `webrtc_media_port`. It has no STUN or TURN, learns your address from your connectivity checks, and ignores `candidate` messages.

The host checks your answer. A changed codec or profile is an error, and the host never falls back to something else.

The host's `control` messages carry a `v`:

```json
{"v":2,"type":"mouse-mode","controllerId":0,"controllerSlot":0,"active":true}
{"v":2,"type":"rumble","controllerId":0,"controllerSlot":0,"strongMagnitude":0.5,"weakMagnitude":0.25,"leftTrigger":0,"rightTrigger":0}
```

`mouse-mode` uses `"v":2`. Magnitudes are 0 to 1.

### Keyframes and logs

A client may ask for a keyframe on the `control` channel, besides an RTCP PLI or FIR. All three set the same request on the host:

```json
{"type":"request-keyframe"}
```

`client-log` is a signalling message, not a DataChannel message. It travels on the WebSocket like any other protocol message, and carries at most 100 lines. The host writes them to its debug log.

```json
{"version":2,"type":"client-log","lines":["decoder reset","frame 1024 late"]}
```

### The extensions object

`capabilities` carries an `extensions` object. Stock clients ignore it. Today it reads:

```json
"extensions": {
  "audioChannels": [2, 6, 8],
  "input": {"label": "input", "version": 1, "keyboard": true, "mouse": true}
}
```

- `audioChannels` lists the channel counts `start-session` accepts in `audio.channels`.
- `input` says the host accepts a client-opened DataChannel with that label. `keyboard` and `mouse` say what it carries.

`version` stays 2. An extension is used only when the client asks for it: a 6 or 8 for `audio.channels`, or an `input` channel.

### Surround audio

The host always offers standard `opus/48000/2` with `stereo=1`. When `start-session` asks for 6 or 8 channels, the host sends the audio as Opus multistream. Chromium refuses `multiopus` in a remote offer, so the client puts it in its own answer:

1. Ask for `"audio":{"channels":6}` or `8`.
2. Set the remote offer, then create the answer.
3. Rewrite the answer's audio `a=rtpmap` to `multiopus/48000/N` and set its `a=fmtp` to libwebrtc's canonical layout:

| Channels | `num_streams` | `coupled_streams` | `channel_mapping` |
| --- | --- | --- | --- |
| 6 (5.1) | 4 | 2 | `0,4,1,2,3,5` |
| 8 (7.1) | 5 | 3 | `0,6,1,2,3,4,5,7` |

For 5.1 the audio lines of the answer then read like this:

```
a=rtpmap:111 multiopus/48000/6
a=fmtp:111 channel_mapping=0,4,1,2,3,5;num_streams=4;coupled_streams=2;minptime=10;useinbandfec=1
```

4. Set that as the local description. The payload type is the one in the offer (111 today). The `minptime` and `useinbandfec` values are yours to choose.

The host takes the stream count, coupled count and mapping from your answer when they are valid for the channel count, and encodes with exactly that layout. A changed channel count or an invalid layout is an error.

A stereo answer to a surround request is refused with an error, never downgraded to a stereo stream. Only receivers that decode `multiopus` can use surround. Chromium-based browsers do. We have not tested the Tizen TV.

### Stereo in Chromium

Chromium decodes Opus as mono unless its own answer's fmtp has `stereo=1`. Add `stereo=1` to the audio `a=fmtp` line of your answer.

### Browser support for video

In our tests, Chrome 155 receives H.265 (our harness starts it with `--enable-features=WebRtcAllowH265Receive`) and Edge 155 has no H.265 receiver. A client that asks for HEVC on a browser without a receiver gets no picture. Check `RTCRtpReceiver.getCapabilities('video')` before you ask.

### The `input` channel

For keyboard and mouse, open a DataChannel labelled `input` after you have signed in. It must be ordered and reliable. The host accepts one, and closes a channel with any other unknown label, or an `input` channel that is unordered or unreliable.

Send each message as a text frame of JSON with `"v":1`. Binary frames are dropped. A message the host cannot parse is dropped and logged.

| Message | Example | Notes |
| --- | --- | --- |
| Key | `{"v":1,"type":"key","code":"KeyA","down":true}` | `code` is the DOM `KeyboardEvent.code`. Modifiers are ordinary keys. An unknown code is dropped and counted in the stream summary. |
| Text | `{"v":1,"type":"text","text":"é"}` | At most 32 characters per message. For input method output and characters with no key. |
| Relative move | `{"v":1,"type":"mouse-move","dx":-3,"dy":5}` | Pixels, for pointer lock. Clamped to 16-bit values. |
| Absolute move | `{"v":1,"type":"mouse-position","x":0.5,"y":0.25}` | A fraction of the video, clamped to 0 to 1. |
| Button | `{"v":1,"type":"mouse-button","button":0,"down":true}` | DOM button numbers 0 to 4: left, middle, right, back, forward. |
| Wheel | `{"v":1,"type":"wheel","dx":0,"dy":120}` | 120 units per notch, with DOM signs: a positive `dy` scrolls down. |

The device's permissions on the Devices page gate keyboard and mouse input as they do for Moonlight. When the channel closes, the stream stops or the session switches, the host releases every key and button it holds for you, so nothing stays pressed. The cursor comes from the captured picture.

### Gamepads

Gamepad messages (`gamepad-connected`, `gamepad-state`, `gamepad-disconnected`) follow the upstream protocol and the TV app. Send them on the `gamepad` channel with `"v":2`; the host accepts `"v":1` too. A client may also announce its pads on `control`. Every pad is announced to the PC as an Xbox pad. Holding Start for 750 ms and releasing it toggles mouse mode for that pad, as described above, and the host reports the change with `mouse-mode`.
