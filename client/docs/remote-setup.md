# Play away from home

[Documentation](index.md)

Long-press a saved PC, choose **PC settings…**, then **Play away from home**. The guide works with any already paired Moonlight-compatible host.

1. Pair on your home network first, check that an ordinary stream works, and keep the PC awake. Waking a sleeping PC from outside the home usually needs a router or another always-on device; this guide does not promise remote Wake-on-LAN.
2. Follow the linked Tailscale installation guide to join the PC and Android device to your own network. The app does not install software, change router settings, request public ports or create an account. Save the PC's VPN address using **Add address** if discovery does not find it.
3. Switch the Android device to another network and run **Test this PC**. This sends five small, bounded HTTPS requests using the saved paired certificate. It does not launch a game, start a stream, run a bandwidth benchmark or change the host. Leaving the screen cancels outstanding requests.

A successful test confirms the paired host's identity and reachability from the device's current network. HTTPS response time includes host request processing, and failed HTTPS checks are not a measurement of video packet loss. Streaming UDP ports and sustained bandwidth still need a real stream. Testing while still at home cannot prove that a remote connection will work.

Hosts that support it (Rubylight) can also report the current Tailscale route: direct, DERP relay, peer relay, or unknown. Only the requesting device's active peer route is used. The response contains no addresses, other peers, names, keys or account details. A relay is useful for connectivity but can add delay; compare a direct connection if available. Other VPNs, subnet routers, idle peers, unavailable Tailscale status, older hosts and unsupported clients show **unknown**, not a guessed LAN/WAN classification. The guide does not send packets to force a direct route.

[Documentation](index.md) · [Pairing](pairing.md) · [Troubleshooting](TROUBLESHOOTING.md)
