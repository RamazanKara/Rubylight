# NVENC fixes
- **NVIDIA: bitrate changes no longer stall or force a keyframe.** A higher bitrate from adaptive bitrate or error correction used to wait for the frames in flight and send a keyframe; on drivers that change the bitrate in place it now applies at once, as on AMD.
- **NVIDIA: a clear message when the encoder sessions are taken.** When another app such as OBS, ShadowPlay or Discord holds the GPU's encoder sessions, the stream now says so instead of suggesting a driver update or sign-in, and the startup check no longer opens a second session beside the first.
- NVIDIA: AV1 now starts on drivers from 522.25, and native 4:4:4, which the colour conversion cannot feed, is refused with a clear message.
