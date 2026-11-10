# YUV 4:4:4 on NVIDIA
<!-- Issue #84. Not run on NVIDIA hardware here: the plan and the hardware matrix mark it Experimental; keep that word out of these notes. -->
- **Native YUV 4:4:4 on NVIDIA GPUs.** A device with its YUV 4:4:4 option on (Moonlight's, or Nightjar's **Sharper colored text (YUV 4:4:4)**) now gets full color detail in H.264 (8-bit) and HEVC (8-bit and 10-bit, SDR and HDR) from an NVIDIA GPU whose video encoder reports 4:4:4 support. Colored text and thin lines keep their color at full resolution. The stream uses the bitrate the device asks for.
- At startup the host encodes a test picture in each 4:4:4 mode and offers only the ones that work. The console's host readiness lists them, for example "YUV 4:4:4 in H.264, HEVC, and HEVC 10-bit."
- AV1 4:4:4 is not available on NVIDIA: the NVIDIA encoder interface Rubylight uses encodes AV1 in 4:2:0 only. Choose HEVC for 4:4:4.
- A device that asks for a 4:4:4 mode the host did not offer gets an error naming the reason, instead of a 4:2:0 stream it did not ask for. Dolby Vision streams in 4:4:4 carry HDR10, since Dolby Vision profile 8 is 4:2:0 only.
- AMD GPUs are unchanged: their encoders make 4:2:0 only, so 4:4:4 from an AMD host still comes through PyroWave or Full chroma.
