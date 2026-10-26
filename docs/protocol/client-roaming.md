# Client roaming: keeping a session when the client's address changes

[Documentation](../index.md) · [Video and latency](../video-and-latency.md#network-drops) · [Host feature support](../PARITY-MATRIX.md)

Status: Experimental, version 1. The wire format is fixed (`rubylight_protocol::roaming`); host and client are in progress.

A Moonlight-protocol session is tied to the client's address. When a phone leaves Wi-Fi for mobile data, or moves to a Wi-Fi network with a different address, the host keeps sending video and audio to the old address and drops control packets from the new one. The stream freezes until a timeout ends it, and the only way back is a new connection.

Roaming lets the client prove its new address to the host, QUIC-style: the host moves the session only after the new address has shown that it holds the session key and that it can receive. Any host can implement it; a client checks the advertisement below, never the host's name.

What already works without it: a VPN that keeps the client's address (Tailscale) is covered by **Ride out network drops**, a media port change on the same IP is followed by the host's pings, and **Recover interrupted streams** reconnects after an address change in seconds. Roaming aims to cut that to under a second, without a new connection.

## Advertising support

The host adds this line to the RTSP DESCRIBE answer when roaming is on:

```text
a=x-rl-roaming:1
```

The number is the highest wire format version the host speaks. A client that does not know the attribute ignores it, like `a=x-rl-control`. Without it the client sends no roaming datagrams. Advertising costs a host nothing: it does nothing until an authenticated PROBE arrives.

## Exchange

Roaming datagrams travel on the host's control UDP port, next to ENet but outside it. Each side recognises them by their first four bytes and handles them before ENet sees them.

1. **PROBE** (client to host). The client sends it from its ENet socket, which is bound to the wildcard address and so leaves through the current route.
2. **CHALLENGE** (host to client). The host answers to the PROBE's source address with a random challenge only the key holder can read. Nothing moves yet.
3. **CONFIRM** (client to host). The client returns the challenge from the same address.
4. **MIGRATED** (host to client). The host has moved the session to the new address and says so.

On commit the host:

- maps the ENet peer's address: datagrams from the new address reach ENet as if from the old one, and ENet's sends to the old address go to the new one;
- moves the session's client IP, so pings, an ENet reconnect and the microphone are matched against the new IP;
- clears the media destinations; the first ping with the session's ping payload from the new IP sets the port, asks for an IDR frame, and resets the link-rate and pacing state for the new path;
- pings the ENet peer so retransmit timers backed off during the outage recover;
- logs `client roamed <old> -> <new>` and answers MIGRATED.

Media keeps today's rule: any port, but only from the validated IP.

## Keys

Both keys come from the per-launch `rikey` (the AES key the client sent in `/launch`), with one AES block encryption each:

- roaming key = AES-128-ECB(rikey, `"RLROAM-KEY-v1\0\0\0"`)
- session tag = the first 8 bytes of AES-128-ECB(rikey, `"RLROAM-TAG-v1\0\0\0"`)

The 16-byte inputs are the ASCII text followed by three zero bytes. The session tag is sent in the clear so the host can find the session; it reveals nothing about the key.

## Wire format, version 1

Every datagram is 52 bytes, all integers little-endian, sealed with AES-128-GCM under the roaming key.

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `0F FE 52 4D` |
| 4 | 1 | version, `1` |
| 5 | 1 | type: `1` PROBE (client to host), `2` CHALLENGE (host to client), `3` CONFIRM (client to host), `4` MIGRATED (host to client) |
| 6 | 2 | reserved, `0` |
| 8 | 8 | session tag |
| 16 | 4 | seq: per sender per launch key, starts at 1, strictly increasing |
| 20 | 16 | GCM tag |
| 36 | 16 | ciphertext of the payload |

- **AAD:** bytes 0 to 19.
- **Nonce** (12 bytes): `type, direction, 0, 0, seq (4 bytes), 0, 0, 0, 0`, where direction is `1` for client to host and `2` for host to client. Type and seq make it unique for each message under one key.
- **Tag order:** the GCM tag comes before the ciphertext, unlike the ciphertext-then-tag output of most AES-GCM APIs.

Read as an ENet header, the magic is peer id 0xFFE with no flags, which no ENet host has; a host without roaming drops these datagrams.

Payloads (16 bytes of plaintext):

| Type | Bytes 0 to 7 | Bytes 8 to 15 |
|---|---|---|
| PROBE | `nonce_c` u64 | `flags` u32, then 4 zero bytes |
| CHALLENGE | `nonce_c` u64 (echoed from the PROBE) | `challenge` u64 |
| CONFIRM | `challenge` u64 | zero |
| MIGRATED | `challenge` u64 | zero |

PROBE flag bit 0 means the client has rebound its media sockets. A receiver ignores flag bits it does not know.

A receiver drops a datagram that is not exactly 52 bytes, has another version or an unknown type, has nonzero reserved bytes, seq 0, an unknown session tag, a GCM tag that does not verify, or nonzero padding in the payload. It never answers a datagram it drops.

## Host rules

- **PROBE** is accepted only for a running session that is not stopping and has an ENet peer. The GCM tag must verify and seq must be greater than the last seq seen from the client. At most 10 PROBEs per second per session are handled, under a global cap as well.
- **Pending candidate.** A PROBE from an address that is not the current one makes it the pending candidate, with a fresh random challenge. A repeated PROBE from the pending address with the same `nonce_c` gets the same challenge again, so PROBEs still in flight on a long round trip do not replace the challenge the client is confirming. A newer candidate replaces an older one; a candidate expires after 3 seconds.
- **CONFIRM** must come from exactly the pending IP and port and carry the pending challenge. At most one commit per second per session.
- **Answers.** A duplicate CONFIRM for the address already committed is answered with MIGRATED again. A PROBE from the current address is answered with CHALLENGE but commits nothing.
- **Timeouts.** Roaming never extends the host's client timeout (`ping_timeout`).

## Client rules

Only when the DESCRIBE answer carried `a=x-rl-roaming:1` and the client's roaming setting is on:

- **Triggers:** the local address toward the host changes (checked every 500 ms, or at once when the platform reports a network change), or the host has been silent for 1 second or more (a NAT rebinding). A network change during an attempt starts it over on the new network, under a new `nonce_c` and with its own give-up time; hearing the host on the old path then no longer ends it.
- **On a trigger:** rebind the video, audio and microphone sockets if their bound IP changed and ping at once; send PROBE every 250 ms (12 times, then once a second); send CONFIRM on a CHALLENGE that echoes the current `nonce_c`; ping at once on MIGRATED.
- **Confirming:** once it sent a CONFIRM, the client stops probing and resends the CONFIRM for the newest challenge every 250 ms until MIGRATED arrives. After 3 seconds without one it probes again. Older hosts answer every PROBE with a new challenge, so probing on would keep replacing the challenge being confirmed whenever the round trip is longer than the probe interval.
- **Giving up:** after the smaller of the drop tolerance and 10 seconds, the stream ends with a clear message as today, or bounded recovery reconnects if it is on. While a longer drop tolerance keeps the stream alive, a silence may start another attempt, at least 10 seconds after the failed one; a network change starts one at once.
- **Seqs:** a reconnect under the same launch key (the same `rikey`) continues the client's seq where the earlier connection left it, since the host keeps the last seq per launch key; a new launch starts at 1.
- **Metered networks:** moving onto a metered network carries the stream on only with "Continue on mobile data" on (default off).

## Security

| Threat | Result |
|---|---|
| Forged PROBE from off the path, or guessing the tag | Sealing needs the rikey, which travels only in `/launch` over mutual TLS. Dropped. |
| Replay on the path | Seq must increase; CONFIRM is bound to the pending address and its challenge. |
| Rewriting the source address to a victim's | The victim never answers the challenge, so nothing moves. An attacker rewriting it to its own address cannot read the challenge either. |
| Reflection or amplification | Each answer is the same size as the request, goes only to an authenticated PROBE and is rate-limited. No media goes to an address that has not confirmed. |
| CPU exhaustion with PROBEs | One GCM open each, no allocation, a global cap, rate-limited log lines. |
| Key or nonce reuse | The roaming key is derived separately from the stream keys; each sender's seq never repeats, and the nonce carries type and direction. |
| Stock clients and hosts | No change: clients ignore the attribute, hosts without roaming drop the datagrams. |

As with QUIC, an attacker that already relays the traffic can make itself the path, which it effectively is already.

Why not simply accept pings or ENet packets from a new address? The ping payload travels in the clear twice a second, so anyone on the path could replay it from any address and turn the host into a reflector of a 50 to 150 Mbps stream aimed at a victim. ENet itself is unauthenticated. A valid tag also proves nothing about the source address: an attacker on the path can drop the real datagram and forward it from a spoofed address first. Only an answered challenge proves the new address can receive.

## Known answers

An implementation must reproduce these (`rubylight_protocol::roaming::vectors`; computed with OpenSSL and checked against RustCrypto in `protocol/client/tests/roaming_vectors.rs`). The client's seq runs PROBE 1, CONFIRM 2; the host's CHALLENGE 1, MIGRATED 2.

| Input or output | Value (hex) |
|---|---|
| rikey | `000102030405060708090a0b0c0d0e0f` |
| roaming key | `6d553d3fa2e47b32ebb1609b11c82df2` |
| session tag | `52d89ae18eeb0232` |
| `nonce_c`, `challenge` | `0x0123456789abcdef`, `0xfedcba9876543210` |
| PROBE (seq 1, flags 1) | `0ffe524d0101000052d89ae18eeb023201000000275f7b4c17bd8fa9b0467587697c1a022c64e82c54fdbd5b9645a4373c755ab8` |
| CHALLENGE (seq 1) | `0ffe524d0102000052d89ae18eeb0232010000002954b77944af32c4e11d538346b6a81ec5035975a9ad5739a4f02740e81f59e9` |
| CONFIRM (seq 2) | `0ffe524d0103000052d89ae18eeb02320200000049a23e8d3d0ab0db81e9b7c106334d238d38b827e46b1abf7a143a1063a29d9c` |
| MIGRATED (seq 2) | `0ffe524d0104000052d89ae18eeb0232020000007e1ab6560eb57e87dd9d21d46c85fb7430202c7441ef5e9e4cbe141ac2ee8d80` |

## Limits

- **The host's address.** A session started on the host's LAN address cannot follow the phone onto mobile data, whatever the host does. A later version lets the client switch to another known host address (remote, IPv6), which needs the host to listen on all addresses.
- **NAT that maps control and media to different public IPs.** Media never resumes, and the stream ends with a clear message.
- **Engines.** Only Nightjar's Rust streaming engine roams; the classic engine does not.
- **Testing.** Phone moves between Wi-Fi and mobile data are tested by hand only, so roaming ships as Experimental.
