## desktop
- Linux with NVIDIA: where NVDEC decodes (no Vulkan video decoding for the stream), CUDA now copies each picture straight into the GPU renderer's Vulkan images instead of through system memory. When that copy cannot be used, the log says why and pictures go through memory as before. The toast asking for a newer NVIDIA driver is gone.
