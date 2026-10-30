## en
- PyroWave streams no longer start at the H.264/HEVC bitrate (20 Mbps at 1080p60), far below what PyroWave needs. With the bitrate on automatic, a PyroWave stream now starts at PyroWave's own default: about 400 Mbps at 1080p60, 1.4 Gbps at 1440p120 and 1.6 Gbps at 4K60. The bitrate setting shows this default while PyroWave is selected, and presets use it too. Android and desktop.
- A bitrate you set below PyroWave's minimum (about 187 Mbps at 1080p60) is raised to the minimum for PyroWave streams, and Nightjar tells you once at stream start. Higher bitrates you set are kept as they are. The mobile data limit still caps the bitrate.
- Nightjar's PyroWave minimum now matches the PC's (it was lower, 133 Mbps at 1080p60).
