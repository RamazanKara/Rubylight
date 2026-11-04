# WebRTC server

[Documentation](README.md) · [Configuration](configuration.md) · [Features](features.md) · [Hardware matrix](hardware-matrix.md)

Rubylight can stream over WebRTC to a Samsung Tizen TV or a browser, alongside the Moonlight protocol. The server is off by default. It speaks the Moonlight WebRTC TV protocol, version 2, which [mlopezsegura](https://github.com/mlopezsegura) designed for the [Tizen TV app](https://github.com/mlopezsegura/Monlight-Tizen-WebRtc-Client) and the [browser client](https://github.com/mlopezsegura/moonlight-webrtc-web). While it is off, no WebRTC socket is open and the Moonlight path is unchanged.

[For users](#for-users) covers setup and use; [For client authors](#for-client-authors) covers Rubylight's behaviour and extensions to the protocol.

## For users

### Turn it on

1. In the console, open **All settings → Network & access** and find the **WebRTC** group.
2. Switch on **WebRTC server**.
3. Save and restart Rubylight when the console asks; the setting takes effect after a restart.

If another program already holds a port, the console shows a `webrtc_server` warning, the log names the error and the server does not run. Rubylight never moves to another port on its own. Pick a free **WebRTC signalling port** instead.

### Ports and firewall

| What | Port | Setting |
| --- | --- | --- |
| Discovery | UDP 8000, always | none |
| Signalling (WebSocket) | TCP 8000 | `webrtc_port` |
| Media, audio and input | UDP 8001 | `webrtc_media_port` |
| Guests of the console's stream | UDP 8002-8005 | `webrtc_media_port` + 1 to + 4 |
| The guest and play pages and their sockets | TCP 47991 | Base port + 2; the guest page answers only while an invite link works, the play page only the local network and Tailscale |

TVs send their discovery broadcast to UDP 8000, so discovery stays there even when you move the signalling port. The reply tells the TV which signalling port to use. The installer's Windows Firewall rule allows `butterpollo.exe` inbound on every port, so an installed host needs nothing more. On a portable host, allow these three ports. The full list of settings is in [Configuration](configuration.md#webrtc-server).

Broadcasts do not cross routers. A TV on another subnet needs the PC's address entered by hand.

### Pair a TV or a browser

1. Open the app on the TV or the browser client. It finds the PC, or you enter its address.
2. The app shows a four-digit PIN.
3. In the console, open **Devices**. The request appears with the device name and a **WebRTC** badge, next to any Moonlight requests.
4. Enter the PIN there and confirm.

The app then holds a client ID and a secret and signs in with them from then on. The secret is stored in `sunshine_state.json` and is never shown in the console. A request lasts 5 minutes and allows one attempt.

### Stream in the console

The console's **Stream** page plays a game from this PC in the browser tab, with no app and no pairing: AV1, HEVC or H.264 at any resolution and frame rate the host offers, stereo sound, keyboard and mouse.

1. Turn on the WebRTC server (see above) and restart Rubylight.
2. Open the console from a device on the local network or Tailscale, sign in, open **Stream** and click **Connect**.
3. Pick a game. Click the picture to play: keys and the mouse go to the PC while it has the focus. Click outside it, or press Ctrl+Alt+Shift+Z, to get them back. **Quit app** ends the stream and the game, **Disconnect** only the stream.

The page signals over the console's own HTTPS port (`wss://`, at `/api/webrtc/signal`), so the WebRTC signalling port is not used, and it signs in with the console session. The browser streams as the device "Console (*your user name*)" on **Devices**; disable it there to stop the console from streaming. Signing out ends the stream within 5 seconds. One tab or browser holds the connection at a time: connecting from another one ends the stream in the first, which says so. The **Picture** panel lists only the codecs this browser receives over WebRTC (`RTCRtpReceiver.getCapabilities`) that the host also offers, with the resolutions and frame rates the host's encoder takes for each. The first time, the page picks a codec the browser decodes in hardware (Media Capabilities), AV1 first, then HEVC, then H.264, at 1080p and 60 fps or the nearest offered; it remembers your choice in the browser. In the project's tests Chromium and Firefox 159 offer AV1 and H.264; HEVC appears only where the browser has an H.265 receiver (see below). The page drops the host's `a=imageattr` line, which is there for Samsung's TVs, before handing the offer to the browser: Firefox refuses it ("Min is not smaller than max").

The browser does not say why the host refused the connection, so the page lists the causes (it asks you to sign in again when the tab's sign-in has lapsed): the tab is not signed in or opened the console from an address the host does not list as its own (`csrf_allowed_origins`), the device is not on the local network or Tailscale, the WebRTC server is off or was turned on without a restart, or the console device is disabled.

### Play in a TV browser

A TV, console or car browser plays games from this PC with no app: it pairs once like a TV, then lists the PC's games and plays one full screen.

1. Turn on the WebRTC server (see above) and restart Rubylight.
2. In the browser, open `https://<PC address>:47991/play` (the guest port, base port + 2). The browser warns that the connection is not private: the page uses the console's own self-signed certificate. Accept it once; the browser remembers it for this address.
3. The page shows a four-digit PIN and the name it pairs as, such as "Xbox" or "Edge on Windows". In the console, open **Devices**, enter the PIN there and confirm.
4. Pick a game with the remote, the arrow keys or a controller (D-pad or left stick, A to start).

Hold Back, Escape or the controller's View (Select) button for a second for the menu: **Back to game**, **Disconnect** (the game keeps running) and **Quit game**. The chip at the top of the games picks the codec; the first time the page picks one as the Stream page does. Browsers allow sound and full screen only after a key press or a click, and a controller press does not count: if the game starts silent, press a key or click once.

The browser keeps its device ID and secret in its site data (`localStorage`), never in the address: clearing the site's data pairs it anew. It is a paired WebRTC device like a TV and gets what a TV gets: the first device paired with the PC may do everything, later ones only watch until you allow more on **Devices**. Unpairing or disabling it there ends its access at once. The page answers only devices on the local network and Tailscale; everyone else gets the same plain "not found" as any other path on the guest port. Only the connection's own address counts, never headers such as `X-Forwarded-For`. A 100.64.0.0/10 address is taken as Tailscale only when it reaches the PC's own Tailscale address, and a connection from the PC's default gateway is refused: a router that rewrites forwarded connections to its own address, or a carrier-grade NAT neighbour, would otherwise look local. Some setups still make an internet connection look local: a tunnel on the PC (Cloudflare Tunnel, ngrok), a reverse proxy, `netsh portproxy`, or a router that rewrites forwards to an address other than its gateway one ([Guests in the browser](browser-guests.md#what-must-be-reachable)). What protects you then is the pairing itself: nothing pairs until you type the PIN in the console, the console and the tray notice show the address each request came from before the name it chose, and a name that contains its own PIN or a digit other than 0-9 is refused. Pair only a device you are holding, from the address you expect. Each device may ask to pair five times a minute, and a PIN that does not match is a warning in the host log.

### Guests

While you stream on the Stream page, **Guests → Invite** makes a link for up to four friends to watch in their own browsers, view only or with their controller, keyboard and mouse, for 1 hour, 24 hours or a time you set. Guests need no app, account or pairing, and connect straight to your PC with no relay: on the local network nothing needs setting up, from the internet forward the guest port, TCP 47991, and UDP 8002-8005 (Tailscale or WireGuard is safer: the guest page's certificate is the console's own). The PC encodes once for everyone, and a slow guest drops frames alone. Setup, ports, the certificate warning and the threat model: [Guests in the browser](browser-guests.md).

### The Devices page

Paired TVs and browsers are listed on **Devices** with a **WebRTC** badge. Disable a device to keep it paired but refuse it. Unpair it to remove it. Either one disconnects the device at once, and ends its stream. At most 32 are kept; the oldest go first.

### What streams

| Part | What you get |
| --- | --- |
| Video | H.264, HEVC and AV1 at 720p, 1080p and 1440p; HEVC and AV1 also at 4K. AV1 appears only when the GPU's AMD encoder supports it. 30, 60, 90 and 120 fps at every resolution, as far as the GPU's encoder can keep up (see below). |
| HDR | HEVC Main10 and AV1 10-bit at 1080p, 1440p and 4K. The host display must already be in HDR. If it is not, or it leaves HDR during the stream, the stream ends with an error. The host never tone-maps to SDR. |
| Audio | Stereo, 5.1 and 7.1. Surround needs a client that asks for it and decodes it; in practice, a Chromium-based browser. |
| Gamepad | Gamepads with rumble. |
| Gamepad as a mouse | Hold Start for 750 ms and release it. The stick moves the pointer, the D-pad scrolls, and A, B, X, LB and RB click. Do it again to turn it off. |
| Keyboard and mouse | From a client that opens the `input` channel (see below), such as the console's Stream page. The browser client does not. |

The host reads how much its encoder can do (AMF's throughput limit per codec) and offers only the frame rates it can encode in real time: a slower AMD GPU may list 4K at 30 and 60 only. A driver that reports no limit gets 60 fps and below. Measured clean on an RX 7900 XT: 90 and 120 fps at 1080p with all three codecs, 1440p120 with HEVC, and 4K120 with HEVC and AV1. Also offered where the encoder reports it can: 720p at 90 and 120, H.264 at 1440p above 60, AV1 at 1440p90 and 1440p120, 90 fps at 1440p and 4K, HDR above 60 fps, GPUs with a lower throughput limit, and drivers that report no limit.

A request for anything the host cannot send is refused with an error. There is no fallback: no SDR for an HDR request, no H.264 for AV1, no stereo for a surround request.

One stream runs at a time. A WebRTC stream and a Moonlight stream exclude each other, and a start that conflicts gets an error instead of taking over.

### Network and safety

Signalling is plain `ws://`, so it is not encrypted. Signing in keeps other devices from using or interrupting the stream; it does not hide what the app and the PC say to each other. The media itself is encrypted (DTLS-SRTP). Use it on a trusted LAN only.

Rubylight serves only peers on the local network: loopback, private (RFC 1918), link-local, unique-local IPv6 and carrier-grade NAT (100.64.0.0/10) addresses. Anything else is closed before its first message and logged. Do not forward these ports on your router. The one exception is guests of the console's stream, who may come from anywhere with a link: their media ports (UDP 8002-8005) and the guest port (TCP 47991, which serves nothing of the console's) may be forwarded; the play page on that port refuses every address outside the local network and Tailscale, and pairs only with the PIN you type in the console ([Play in a TV browser](#play-in-a-tv-browser) has the setups that make an internet address look local). Never forward the console's port, TCP 47990 ([Guests in the browser](browser-guests.md#what-must-be-reachable)).

## For client authors

Rubylight implements protocol version 2 as the upstream [protocol document](https://github.com/mlopezsegura/Monlight-Tizen-WebRtc-Client/blob/main/docs/protocol.md) describes it: discovery, pairing, authentication, apps, `start-session`, `switch-session`, the stop messages and the host's offer. Read that for the messages. This part covers the host behaviour a client must handle and Rubylight's extensions. A stock client sees exactly protocol v2.

### The host's offer

The host sends the offer. It carries one video codec at a fixed payload type, one Opus audio track, and two DataChannels it creates itself:

| Label | Kind | Use |
| --- | --- | --- |
| `control` | ordered, reliable | Host to client: `rumble` and `mouse-mode`. Client to host: `request-keyframe`. |
| `gamepad` | unordered, no retransmits | Client to host: gamepad messages. |

The host is ICE-lite. Its only candidate is its own address plus `webrtc_media_port`. It has no STUN or TURN, learns your address from your connectivity checks, and ignores `candidate` messages.

The host checks your answer. A changed codec or profile is an error, and the host never falls back to something else.

The video track offers NACK with RTX, so a lost packet is resent instead of costing a keyframe. The host paces its packets at a rate above the stream's bitrate, so a keyframe does not leave as one burst. The HEVC `level-id` and AV1 `level-idx` follow the mode, the rate and the bitrate (up to 5.2 for 4K120).

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

`capabilities` carries an `extensions` object. Stock clients ignore it. In 3.0 it reads:

```json
"extensions": {
  "audioChannels": [2, 6, 8],
  "input": {"label": "input", "version": 1, "keyboard": true, "mouse": true},
  "codecFrameRates": [
    {"width": 1280, "height": 720, "codec": "h264", "frameRates": [30, 60, 90, 120]},
    ...
  ]
}
```

- `audioChannels` lists the channel counts `start-session` accepts in `audio.channels`.
- `input` says the host accepts a client-opened DataChannel with that label. `keyboard` and `mouse` say what it carries.
- `codecFrameRates` lists, for each mode and codec, the frame rates that codec takes on this host's encoder. A `videoModes` entry lists only the rates every one of its codecs takes, so any listed rate works with any listed codec. When the encoder's limits differ by codec, a codec may take more rates than its mode lists, or a codec that cannot take 60 fps at a mode may be left out of the mode and appear only here. The host accepts every pair listed here.

`version` stays 2. An extension is used only when the client asks for it: a 6 or 8 for `audio.channels`, an `input` channel, or a rate from `codecFrameRates`.

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

4. Set that as the local description. The payload type is the one in the offer (currently 111). The `minptime` and `useinbandfec` values are yours to choose.

The host takes the stream count, coupled count and mapping from your answer when they are valid for the channel count, and encodes with exactly that layout. A changed channel count or an invalid layout is an error.

A stereo answer to a surround request is refused with an error, never downgraded to a stereo stream. Only receivers that decode `multiopus` can use surround. Chromium-based browsers do; surround on the Tizen TV app has not been tested.

### Stereo in Chromium

Chromium decodes Opus as mono unless its own answer's fmtp has `stereo=1`. Add `stereo=1` to the audio `a=fmtp` line of your answer.

### Browser support for video

In the project's tests, Chrome 155 receives H.265 (the harness starts it with `--enable-features=WebRtcAllowH265Receive`) and Edge 155 has no H.265 receiver. A client that asks for HEVC on a browser without a receiver gets no picture. Check `RTCRtpReceiver.getCapabilities('video')` before you ask.

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

The device's permissions on the Devices page gate keyboard and mouse input as they do for Moonlight. A guest of the console's stream gets the `input` channel only with an invite for keyboard and mouse, and its input is checked against both the invite and the console device's permissions ([Guests in the browser](browser-guests.md#threat-model)). When the channel closes, the stream stops or the session switches, the host releases every key and button it holds for you, so nothing stays pressed. The cursor comes from the captured picture.

### Gamepads

Gamepad messages (`gamepad-connected`, `gamepad-state`, `gamepad-disconnected`) follow the upstream protocol and the TV app. Send them on the `gamepad` channel with `"v":2`; the host accepts `"v":1` too. A client may also announce its pads on `control`. Every pad is announced to the PC as an Xbox pad. Holding Start for 750 ms and releasing it toggles mouse mode for that pad, as described above, and the host reports the change with `mouse-mode`.
