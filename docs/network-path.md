# Remote connection route

The paired-client HTTPS endpoint `GET /api/network-path` helps a client explain an away-from-home connection. It uses the streaming HTTPS port, not the administration port. The client needs a valid, enabled paired certificate and a view permission. HTTP and administration tokens do not grant access. `serverinfo` advertises `NetworkPathVersion=1` only to an authenticated viewer.

```json
{"version":1,"kind":"direct"}
```

`kind` is `direct`, `relay` (Tailscale DERP), `peer_relay`, or `unknown`. The host runs the standard installed `Program Files/Tailscale/tailscale.exe status --json` on demand, with a three-second timeout and a 1 MiB output limit. Calls share a five-second cache; another peer cannot receive the cached peer's route. Busy requests return unknown. Responses carry `Cache-Control: no-store`.

Only an active Tailscale peer with an exact match to the authenticated connection's source IP is classified. Peer-relay status takes priority, then the active direct address, then DERP. Inactive status can be stale, so it is unknown. Subnet-routed peers, absent or nonstandard Tailscale installations, stopped backends and malformed status are also unknown. The endpoint does not report all peers or disclose names, addresses, device IDs or keys. It neither configures the VPN nor sends probes to change its route.

This is a route hint, not a reachability or latency guarantee. A client should pair locally first, verify the stored host certificate over HTTPS after moving to another network, and clearly separate HTTPS response time from streaming UDP loss and input-to-screen latency. Remote Wake-on-LAN still needs suitable home-network support.

Unit tests use bounded JSON fixtures and mocked endpoint requests; they do not invoke Tailscale or start a stream.

[Documentation](README.md) · [HTTP API](api.md) · [Getting started](getting-started.md)
