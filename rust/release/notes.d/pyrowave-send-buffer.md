# PyroWave at very high bitrates
- Above about 840 Mbps of pacing, the host's video socket buffer grows with the pace to hold 10 ms of traffic, up to 8 MB, instead of a fixed 1 MB. Lower bitrates keep the 1 MB buffer.
- New setting **Settings → Video → Codecs → Overlap PyroWave frames** prepares the next PyroWave frame while the GPU still encodes the current one, with two frames on the GPU at once. It is off by default: at 4K and 120 fps it measured no lower latency.
- PyroWave pictures go from the encoder's output straight into network packets, one full-frame copy fewer per frame.
