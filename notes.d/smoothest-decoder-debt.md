## en
- Smoothest and the frame rate limit no longer keep a frame or two of extra decoder delay for up to a minute after a stream starts or recovers (seen on the NVIDIA Shield). Once the extra delay has lasted about 1.5 seconds, Nightjar skips one waiting frame at a time until it is gone, then shows every frame again.
