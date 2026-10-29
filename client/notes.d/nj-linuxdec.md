## en
- Desktop (Linux): hardware decoding now also works on ARM handhelds and boards (V4L2, H.264 and HEVC) and on NVIDIA GPUs (NVDEC). Nightjar tries VAAPI first, then V4L2, then NVDEC, and only then decodes in software; the log says why each one it skipped did not start.
- Desktop (Linux): VAAPI pictures go straight from the decoder to the screen without a copy through the CPU (with the OpenGL renderer, for 8-bit streams).
