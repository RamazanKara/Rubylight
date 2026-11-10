## en
- When HDR takes priority over YUV 4:4:4, the notice now says so.

## desktop
- Windows: YUV 4:4:4 decodes in hardware on GPUs whose decoder supports HEVC 4:4:4 (8-bit and 10-bit). On GPUs without one, the stream uses 4:2:0 and says why.
- With "Sharper colored text (YUV 4:4:4)" and "Less color banding (10-bit SDR)" both on, Nightjar asks for HEVC 4:4:4 in 10-bit where the PC and this device support it.
- Linux: 10-bit 4:4:4 pictures from NVIDIA's decoder are drawn correctly.
