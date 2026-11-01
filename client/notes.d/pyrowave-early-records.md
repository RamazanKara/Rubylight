## en
- PyroWave decodes sooner on Android: with a Rubylight PC, each frame goes to the GPU decoder while it is still arriving, so only the last bit of the frame and the decode are left when it is complete. On a Retroid Pocket 6 at 1080p60 and 400 Mbps, the time from a frame's last packet to its decoded picture fell from about 3.5 to 3.0 ms (p95 from 6.0 to 3.4 ms).
