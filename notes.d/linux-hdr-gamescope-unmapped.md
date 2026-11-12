## desktop
- Linux under gamescope (Steam Deck Game Mode and gamescope sessions): an HDR stream now presents as HDR10 where gamescope offers it, as on a desktop, instead of scRGB.
- Linux: when VA-API pictures cannot be shared with the GPU renderer, the stream is decoded with Vulkan video where the graphics driver supports it, instead of copying every picture through memory, which at 4K took longer than a frame.
- Linux, HDR: a stream with the statistics overlay, a message or the stream menu on screen no longer takes two extra full-screen passes for each picture, which at 4K could leave a small GPU unable to show every picture while the overlay was on. On an Intel Iris Plus 655 at 4K the overlay took about 2.5 ms of GPU time for each picture and now takes about 0.2 ms.
- The overlay's Render no longer counts the present call, where Nightjar waits for the graphics driver, the display and the compositor (gamescope, for one). That wait has its own line, Present wait.
- Windows and Linux packages ship FFmpeg 9.0.2 instead of 8.1.3.
- Linux, built from source against FFmpeg 9 (as Arch Linux ships it): VA-API pictures are shared with the GPU renderer and Vulkan video decodes, as with FFmpeg 7.1 and 8. Before, such a build copied every picture through memory.
