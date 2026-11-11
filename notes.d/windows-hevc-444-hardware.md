## desktop
- Windows: YUV 4:4:4 in HEVC now decodes in hardware on GPUs that support it, such as NVIDIA RTX and recent Intel GPUs. Until now the FFmpeg that Nightjar ships on Windows had no hardware path for it, so 4:4:4 stayed unavailable even on those GPUs.
- Windows: when 4:4:4 can't be decoded, Nightjar no longer suggests switching to the same GPU it already uses (seen on laptops whose GPU Windows lists more than once).
