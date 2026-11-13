# Play over the internet

[Documentation](README.md) · [Configuration](configuration.md#play-over-the-internet) · [Privacy](privacy.md#third-party-services)

Devices reach your PC from outside your network without forwarding ports by hand, and without any server run by the Rubylight project: no relay carries your stream and no rendezvous server knows your devices. Two settings are involved:

- **Automatic port forwarding** (`upnp`, off by default) asks your router to forward the streaming ports: through UPnP IGD, else PCP, else NAT-PMP, for IPv4, and opens IPv6 firewall pinholes through UPnP IGDv2 or PCP. Stock Moonlight benefits as well: it reaches a forwarded port at the PC's `ExternalIP`.
- **Play over the internet** (`internet_play`, off by default) adds a direct path for when no port can be forwarded, such as behind carrier-grade NAT: Nightjar and the host find each other through the public BitTorrent DHT and punch a UDP path through both routers.

## The order a device tries

1. **The local network.** At home nothing else is tried and nothing is added to the stream.
2. **The PC's IPv6 address.** No router mapping is needed where the router lets it in (or the host opened a pinhole).
3. **A forwarded IPv4 port**, at the PC's public address, when every streaming port is forwarded under its own number.
4. **A punched path.** Only Nightjar, only with a host that offers it (`RustHostInternetPlayVersion` 1 in a paired serverinfo), and only after the device was on the PC's network once with the option on.

The PC's line in Nightjar names the path in use (Internet (IPv6), Internet (forwarded port) or Internet (direct path)). When none works, it lists every path and why it failed, such as "IPv6: no answer; a direct internet path: both routers change ports per destination".

## What the host does

- **Port mapping.** With `upnp` on, every streaming port (HTTP, HTTPS, RTSP, video, audio, control, microphone) is asked for. Leases are renewed every minute and removed when the host stops. The console's **Settings → Network → Internet status** shows the method that worked, each port mapped or refused and why, the public address, the IPv6 pinholes, and notes for carrier-grade NAT (the router's own address is not public) and a second router in front.
- **Public address.** With `internet_play` on, the host asks public STUN servers for its public address every five minutes (`internet_play_stun_servers`, default `stun.moonlight-stream.org:3478` and `stun.cloudflare.com:3478`) and classifies the router: cone (one mapping for every destination) or symmetric. Paired serverinfo reports `ExternalIP` when either setting is on and the address is known.
- **Keys for each device.** Each paired device gets a random 32-byte secret, stored with the device on the host and fetched by that device over the paired HTTPS connection (`GET /internet-play/v1`). Unpairing removes it.
- **Rendezvous.** For each device, the host publishes a record on the DHT every 30 minutes and on change, and looks for the device's record every ten seconds.
- **Tunnels.** A punch authenticated with a device's keys opens a path. The host connects tunnelled HTTP, HTTPS and RTSP to its own ports on loopback from a source address of the device's own (127.77.x.y), so sessions of two devices stay apart, and forwards media datagrams to its media ports.

## Rendezvous on the DHT

The records are BitTorrent DHT mutable items (BEP 44), stored by public DHT nodes for about two hours. The host and the device each publish one and read the other's:

- **Keys.** HKDF-SHA256 over the device secret (salt `rubylight internet play v1`) derives an Ed25519 signing key per side, a record key, and a salt per side and hour. The item's target is SHA-1 of the public key and the salt, so the target changes every hour and nobody without the secret can link two hours, a device and its PC, or find either.
- **Contents.** Sealed with ChaCha20-Poly1305 under the record key: the side's role, time, session ID (random per run), the peer session it answers, its NAT type, up to eight public endpoints and, from the host, its port numbers. A node storing it sees only its size, the signature and the sequence number (the publication time).
- **Lookups.** Iterative `get_peers`-style lookups (`find_node`, `get`, `put` with a write token) with `ro=1` (BEP 43): neither side answers DHT queries or stores anyone else's items. The bootstrap nodes are `router.bittorrent.com:6881`, `dht.transmissionbt.com:6881`, `router.utorrent.com:6881` and `dht.libtorrent.org:25401`.

## Punching and the tunnel

Both sides send authenticated punches to every endpoint the other published, every 100 ms for 3 s and then every 500 ms, for up to 30 s. A side behind a symmetric NAT is also tried at the next eight ports. The first authenticated answer fixes the path, including a peer address neither side published (peer-reflexive).

- **Wire format.** Each datagram: magic `A7 52 4C`, version 1, kind (punch, punch ack, stream, media, close, keepalive), channel, an 8-byte path tag, an 8-byte sequence number, and a ChaCha20-Poly1305 payload whose nonce is the sequence number. Keys are per direction and per pair of sessions. A 64-entry window refuses replays. Keepalives every 10 s; a path silent for 45 s is closed.
- **TCP channels.** HTTP, HTTPS and RTSP run over a small reliable stream layer (selective acknowledgements, RFC 6298 retransmission timer, fast retransmit, up to 256 segments of 1100 bytes in flight, 512 KiB send buffer per stream). HTTPS stays end-to-end TLS with the certificate pinned at pairing.
- **Media.** Video, audio, control and microphone datagrams go straight over the punched path, one sealed datagram each, without the stream layer. Streams over it use Moonlight's remote packet size (1024 bytes) so the sealed datagrams fit.
- **The device's side.** The tunnel binds the PC's port numbers on a loopback address of the device (127.78.x.y from the PC's ID; 127.0.0.1 on macOS, which binds only that). Nightjar then talks to that address as to the PC, so pairing, the library and both streaming engines are unchanged.

Punching works for IPv4 when at least one side has a cone NAT, or both are port-restricted cones. Two symmetric NATs, or a symmetric NAT opposite a port-restricted one, cannot be punched; then only a forwarded port or IPv6 reaches the PC.

## Limits

- A tunnelled device reaches the host's ports from this PC, yet the host treats it as a device on the internet: every connection and media socket the tunnel opens is marked as tunnelled before it sends, and 127.77.0.0/16 and 127.78.0.0/16 always count as the internet. It gets `wan_encryption_mode`, never `lan_encryption_mode`, none of what serverinfo shows only programs on this PC (session counts, the host's profile), no browser-on-this-PC trust and no LAN-only guest play. The path is sealed with the device's keys.
- The host keys sessions by the device's source address (127.77.x.y). A host that listens on one address, not every address, connects every tunnel from that address, so it serves one device over the internet at a time.
- The host polls each device's record every ten seconds while the option is on: a few small DHT packets per device.
- Away from home, a PC that is off is tried over the DHT at most once a minute.
