## desktop
- Linux under gamescope (Steam Deck Game Mode and gamescope sessions): an HDR stream now presents as HDR10 where gamescope offers it, as on a desktop, instead of scRGB.
- Linux: when VA-API pictures cannot be shared with the GPU renderer, the stream is decoded with Vulkan video where the graphics driver supports it, instead of copying every picture through memory, which at 4K took longer than a frame.
