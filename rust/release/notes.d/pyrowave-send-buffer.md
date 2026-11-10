# PyroWave at very high bitrates
<!-- The first two bullets wait for their A/B (send drops at 1.4 Gbps; encode overlap on and off); drop or reword either if it shows no gain. -->
- **Fewer packets dropped by the host at PyroWave's highest bitrates.** Above about 840 Mbps of pacing, the host's video socket buffer now grows with the pace to hold 10 ms of traffic, up to 8 MB. With the fixed 1 MB buffer, a short delay in the host's network adapter at 1.4 Gbps could make Windows refuse a whole batch of packets, and the client lost that picture. Lower bitrates keep the 1 MB buffer.
- **PyroWave prepares the next frame while the GPU still encodes the current one.** Two frames are on the GPU at once, each with its own copy of the picture. **Settings → Video → Codecs → Overlap PyroWave frames** turns it off to encode one frame at a time, as before.
- PyroWave pictures go from the encoder's output straight into network packets, one full-frame copy fewer per frame.
