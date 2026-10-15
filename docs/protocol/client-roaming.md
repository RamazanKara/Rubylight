# Proposal: keeping a session when the client's address changes

[Documentation](../index.md) · [Video and latency](../video-and-latency.md#network-drops) · [Host feature support](../PARITY-MATRIX.md)

Status: proposal for host authors. Nothing here is implemented in a host, and Nightjar adds no client behaviour for it yet.

## Problem

A Moonlight-protocol session is tied to the client's address. When a phone leaves Wi-Fi for mobile data, or moves to a Wi-Fi network with a different address, the host keeps sending video and audio to the old address and drops control packets from the new one. The stream freezes until a timeout ends it, and the only way back is a new connection (Nightjar's bounded reconnect does this when **Recover interrupted streams** is on).

Nightjar already covers the cases where the address stays the same: roaming inside one Wi-Fi network, a short dropout, or a VPN that keeps its address while it changes paths. **Ride out network drops** lets the client wait up to 60 seconds for those. What remains needs the host.

## What the host would do

1. **Control stream (ENet).** Accept a datagram for an existing peer from a new source address and port when it decrypts and authenticates with that session's control key and carries the next expected sequence number. Then move the peer to the new address. Unauthenticated datagrams from a new address keep being dropped as today, so an attacker on the path cannot take the session over. Sessions without control-stream encryption do not migrate.
2. **Video and audio (RTP).** Hosts already learn where to send media from the client's ping packets. Accept a ping from a new address when it carries the session's ping payload (the per-stream `X-SS-Ping-Payload` from the RTSP SETUP answer, or an equivalent token), and switch the destination to that address. Ignore pings from a new address without the token.
3. **Rate and scope.** Allow a migration at most once per second per session, and only for a session that is still running. Log each migration with old and new addresses so users can see it happened.
4. **Timeouts.** Keep the host's client timeout (for example `ping_timeout`) as the bound for how long a silent client is kept. Migration does not extend a session that stays silent.
5. **Advertise it.** Report the capability where the client already reads host features, for example a `ClientRoaming` element in serverinfo or an attribute in the RTSP DESCRIBE answer. The client must not rely on migration without it.

## What the client would do

Only once a host advertises the capability, and only behind `HostCapabilities` (never a host-name check):

1. Watch for a change of the active network (Android `ConnectivityManager.NetworkCallback`).
2. Re-create the video and audio sockets on the new network if they were bound to the old address, and send pings at once so the host learns the new address.
3. Keep the control connection: ENet sends from the new address on its own once the old route is gone.
4. If no video arrives within the drop tolerance, end the stream with a clear message as today, and let bounded recovery reconnect if it is on.

The client side stays off by default, like the drop tolerance, until it has been tested on real networks.

## Open questions

- Whether hosts that bind media sockets to one interface can send from another without a restart.
- How the host should treat two addresses sending valid traffic at once (a phone briefly on Wi-Fi and mobile data together). Proposed: the newest authenticated address wins.
- Whether NAT on mobile networks changes the client's port often enough to need a lighter check than full re-authentication.
