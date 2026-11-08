## en
- Decoder fixes: a frame the decoder refused no longer makes smooth pacing wait for it, and HEVC and AV1 get their own decoder's slice and submit settings.
- The Vulkan renderer lets the GPU wait for each decoded frame instead of a CPU thread, where the device supports it, and Android's performance hints now get the CPU time the renderer used rather than the time it waited.
- New Debug settings, off by default: "Speed up frame hand-off", "Ready the decoder for folding" and "PyroWave early screen image".
