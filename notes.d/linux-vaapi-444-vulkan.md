## desktop
- Linux with Intel graphics: full chroma (4:4:4) streams decode in hardware with VAAPI when the GPU renderer is used. The decoder's single-plane 4:4:4 pictures go straight into the renderer's Vulkan images instead of being refused. Where they cannot be mapped, the log says why and 4:4:4 is not offered, as before.
