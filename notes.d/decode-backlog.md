## en
- A decoder too slow for the stream no longer shows as network loss: frames dropped because the decoder fell behind get their own "Decoder drops" line on the overlay, and the loss figure, the automatic bitrate and the stream doctor count only what the network lost.
- PC's frame times no longer hold pictures back while the decoder stays behind: each picture shows as soon as it is decoded, instead of the pacing learning the backlog as delay and holding every frame. A burst of frames from the network is still spread out as before.

## desktop
- Linux, Flatpak included: the connection to the PC is found without the ip tool, so Wi-Fi (Steam Deck) gets Wi-Fi pacing again and a VPN gets packets that fit it, instead of every Flatpak connection reading as "wired or unknown".
- The stream doctor leaves out the stream's first two seconds and counts a single slow moment once, so the first picture or a switch to HDR no longer reports the PC as the bottleneck.
