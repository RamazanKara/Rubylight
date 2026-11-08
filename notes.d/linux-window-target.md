## desktop
- Linux: the GPU renderer draws each frame straight into the window's swapchain instead of a back buffer that was then copied to it, one full-frame copy less per picture. Screenshots, and HDR10 output's PQ pass, keep the back buffer.
