# Guests in the browser

[Documentation](README.md) · [WebRTC server](webrtc.md) · [Configuration](configuration.md)

While you stream a game in the console's **Stream** page, up to four friends can watch it in their own browsers, and play along with a controller or a keyboard and mouse if you let them. They get a link, open it, type a name and see and hear your stream. They need no app, no account and no pairing. There are no relay servers in between: every guest connects straight to your PC.

To have a paired Nightjar or Moonlight device join the game another device plays, see [Guests from other devices](multiclient.md).

## Invite someone

1. Turn on the WebRTC server and stream from the console's **Stream** page ([Stream in the console](webrtc.md#stream-in-the-console)).
2. Under **Guests**, click **Invite**. Pick what guests can do and how long the link works, then **Create link**:
   - **View only**: picture and sound. Whatever their browser sends is dropped on the PC.
   - **Controller** (the default): their gamepads, each on a virtual pad of its own.
   - **Controller, keyboard and mouse**: also their keys and mouse, which control this PC as yours do. Keep these for people on your own network or VPN ([The certificate warning](#the-certificate-warning)).
   - The link expires after 1 hour, 24 hours (the default) or a time you set, from 5 minutes to 7 days.
   - **Address** is where guests open the link: the address you opened the console at (your PC's LAN address when you opened it on the PC itself), on the guest port, TCP 47991. For a guest on the internet, put your router's public address or a DNS name for it there; without a port the link uses 47991. The console remembers what you type.
3. Copy the link and send it.

A guest who opens the link while you are not streaming sees "Waiting for the host" and joins when your stream starts. When your stream ends, every guest's ends with it; their pages wait for your next stream and join it while the link works.

**Guests** lists who is watching and every link that still works. **Kick** disconnects a guest; the link still works, so they can come back. **Revoke** withdraws a link: it opens nothing from then on, and whoever joined with it is disconnected. An expired link lets nobody new in; guests already watching stay until you kick them, revoke the link or stop streaming, and the link stays listed as **Expired** until they have gone, so you can still revoke it. Rubylight keeps links in memory only, so restarting it (or the PC) revokes them all. It keeps at most 16 links at once.

The guest port answers only while a link works or a guest is watching. Before your first link, and once every link has expired or been revoked and the last guest has left, it answers every request with "not found".

Guests see your stream exactly as you do: the same codec, resolution and frame rate, picked on your Picture panel. A guest's browser has to receive that codec over WebRTC: in the project's tests Chromium and Firefox receive AV1 and H.264, and HEVC only where the browser has an H.265 receiver ([Browser support for video](webrtc.md#browser-support-for-video)). A guest whose browser can't play it gets a message saying so.

## What must be reachable

On the local network, Tailscale or a VPN, nothing needs setting up: guests open the link at your PC's address.

From the internet, these must reach the PC. With UPnP on (`upnp`) and a router that allows it, the PC forwards them itself, and removes them when it stops; otherwise forward them on your router:

| What | Port | Notes |
| --- | --- | --- |
| The guest page and its socket | TCP 47991 | The guest port (base port + 2). It serves `/guest/` while a link works, and `/play`, the [TV browser play page](webrtc.md#play-in-a-tv-browser) and its pairing socket, to the local network and Tailscale only. |
| Each guest's picture, sound and input | UDP 8002-8005 | `webrtc_media_port` + 1 to + 4: one port per guest. |

Never forward TCP 47990, the console's port, nor UDP 8000 or 8001 (the TV protocol and your own stream). The console tells your local network from the internet only by the address a connection comes from, and a router that rewrites source addresses, a tunnel (Cloudflare Tunnel, ngrok), a reverse proxy or `netsh portproxy` makes internet connections look local. The guest port has no console behind it, but it has the play page, which tells the local network from the internet the same way. Rubylight catches two of these setups: a router that rewrites forwarded connections to its own address (the PC's default gateway) is refused, and a 100.64.0.0/10 address counts as Tailscale only on the PC's Tailscale address, not when a neighbour behind your ISP's carrier-grade NAT reaches the forwarded port. A tunnel or reverse proxy on the PC (Cloudflare Tunnel, ngrok), `netsh portproxy`, or a router that rewrites to another address still makes an internet browser look local to the play page. It can then ask to pair, but nothing pairs until you type the PIN it shows in the console, which shows the address each request came from ([Play in a TV browser](webrtc.md#play-in-a-tv-browser)). Type a PIN only for a device you are holding, from the address you expect.

When a guest joins, the PC looks up the link's address (only the one you typed, never one a guest sends) and offers it to the guest's browser beside its own, so a forwarded UDP port is found.

Rubylight uses no STUN or TURN server and no relay. Each guest's browser connects to the PC's UDP port; the PC learns the guest's address from that connection. A guest behind its own router needs no setup. A PC that can't take incoming connections (behind carrier-grade NAT, or a router you can't configure) can't have guests from the internet; Tailscale gets around that. A guest whose browser gets no answer on the UDP port sees that within 8 seconds, with the port and what to ask you for ([what each message means](troubleshooting.md#the-browser-stream-or-a-guest-link-fails-to-connect)).

## The certificate warning

The guest port uses the console's own certificate, which browsers on the PC trust and other devices don't ([Getting started](getting-started.md)). A guest therefore sees a certificate warning when they open the link, and that warning means their browser cannot tell your PC from a machine in between. Whoever sits between you and the guest (on public Wi-Fi, a network you don't run, or the internet) can show the same warning with a certificate of their own, read the token as the page sends it and use the link as your guest: watch, play, and with keyboard and mouse control your PC. The stream's own encryption (DTLS-SRTP) does not help, because its keys travel over that same connection.

- On your own network, or over Tailscale or WireGuard, the path between you is yours: clicking through the warning is reasonable.
- For guests on the internet, connect them over Tailscale or WireGuard, which authenticate both ends, rather than a forwarded port. Rubylight cannot use a certificate of your own for the guest port.
- Give keyboard-and-mouse links only to people on your network or VPN. The invite dialog says what such a link can do when you pick it.

## Threat model

What a link gives: the guest page and its socket, and through them one seat in your running stream with the link's permission. Nothing else.

- **No way into the console.** The guest page is its own small build served under `/guest/`; no path under it serves a page or file of the console's, and its Content-Security-Policy lets it load only its own files and talk only to this PC. Its socket takes three messages (join with the token, the answer to the stream offer, leave) and nothing of the console's: no app list, no starting or stopping apps, no settings. A signed-in console session in the same browser grants a guest socket nothing. Making, listing, revoking and kicking take the console's sign-in and its CSRF token like every other console action, and API tokens cannot reach them.
- **The token.** 128 random bits, in the link's fragment (after `#`), which browsers never send in a request, so no server log, proxy or referrer sees it. The page sends it once, as its socket's first message. The host keeps only its SHA-256 hash, compares it against every link's in constant time, and never shows it again after the dialog that made it. Anyone who has the link can use it until it expires or you revoke it: send it only to the people you mean.
- **Guessing.** Five wrong tokens per minute from one address (one /64 for IPv6) and that address waits, before any token is looked at; a right token waits too. A page that sends no token within 5 seconds, or something else first, counts as a wrong one. At most 16 guest sockets wait for their token at a time, 4 from one address; a guest who joined no longer counts, so pages that never join cannot lock out the guests watching. 4 guests watch at once.
- **Permissions.** The host checks every input message against the link's permission, whatever the page sends. View only: nothing the guest sends reaches the input worker. Controller: gamepads only, no keyboard, mouse or gamepad mouse mode. Keyboard and mouse also need your console device's own permission on **Devices**.
- **Your input comes first.** Guests' input waits in a queue of its own that the PC takes from only when yours is empty. With 64 of a guest's messages waiting, its mouse motion is dropped and a pad's or the pointer's newest state replaces the one still waiting; keys, buttons, text and pads plugging in or out keep queuing, up to 256. Past that the PC lets go of everything the guest holds and unplugs its pads, so nothing stays pressed on your PC. Each guest plugs in at most 4 pads, and the guests together leave 4 of your stream's 16 controller slots to you.
- **What it can't protect.** A guest with keyboard and mouse controls this PC as you do, outside the game too: give that only to people you would hand your keyboard. Any guest can record what they watch. A guest's browser sees your stream's picture, which shows whatever is on your screen.
- **Your stream comes first.** The PC encodes once for everyone. Each guest has its own connection, packetizing, encryption and pacing, so a guest on a poor link drops frames alone and never delays yours. With guests watching, each frame is copied once into a buffer they share, on the encoder's side, before yours is queued. A guest who lost its picture asks for a keyframe; the guests between them get at most one every 2 seconds, and each is a keyframe in your stream too.
- **Messages.** A guest's page gets short messages; the details of what went wrong stay in the host's log.

Text and voice chat are not part of this; use a voice app beside it.

## For developers

The owner's API, behind the console sign-in (`rust/host/src/web/guests.rs`):

| Request | Body | Answer |
| --- | --- | --- |
| `GET /api/guests` | | `{"streaming", "max_guests", "port", "invites": [{"id", "permission", "created", "expires"}], "guests": [{"id", "name", "permission", "invite", "since"}]}`; `invites` includes expired links a guest still watches with |
| `GET /api/guests/addresses` | | `{"addresses": [...]}`: this PC's LAN addresses |
| `POST /api/guests/invite` | `{"permission": "view" \| "controller" \| "controller-keyboard-mouse", "seconds": 300..604800, "address": "<host>[:port]"}` (`address` optional) | `{"invite": {...}, "token"}`; 409 with 16 links open |
| `POST /api/guests/revoke` | `{"id": "<invite id>"}` | `{}`, also for an expired link someone still watches with; 404 otherwise |
| `POST /api/guests/kick` | `{"id": <guest id>}` | `{}`; 404 when that guest is not watching |

The guest socket, `wss://<host>:47991/guest/signal` with subprotocol `rubylight.guest.v1` and the page's own origin (`rust/host/src/webrtc/guests.rs`), takes JSON text messages:

- Page to host: `{"type":"join","token","name"}` first (within 5 s), then `{"type":"answer","sdp"}` (within 15 s), and `{"type":"leave"}`.
- Host to page: `{"type":"offer","sdp","name","permission","codec","width","height","fps"}`, `{"type":"streaming"}` once the media connected, and as the last message either `{"type":"ended","reason":"kicked"|"revoked"|"ended","message"}` or `{"type":"error","code","message"}` with `code` one of `rate-limited`, `invalid-link`, `not-streaming`, `full`, `busy`, `timeout`, `invalid-message`, `unsupported`, `connection-lost`, `internal`.

The offer is the owner's stream's: the same codec and Opus layout, with the `control` and `gamepad` DataChannels of [The host's offer](webrtc.md#the-hosts-offer). A guest with keyboard and mouse opens the `input` channel ([The input channel](webrtc.md#the-input-channel)); the host accepts it only for that permission. The guest page is built by `rust/web/vite.guest.config.ts` into `dist/guest`.
