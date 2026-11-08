## desktop
- Linux: AMD graphics decode each picture with the driver's low-latency mode, so pictures leave the decoder sooner.
- Linux: with V-Sync off, pictures go to the display at once, as on Windows, instead of waiting in the compositor's queue. A Variable refresh rate stream on Wayland keeps its tear-free presentation. The log names the presentation mode the display actually uses.
- Linux: on Wayland, the log now records when each picture reached the screen and ends each stream with a summary, and the timing CSV gains the matching columns.
- Linux: Vulkan Video, PyroWave and 10-bit or full-chroma software-decoded pictures are converted and scaled straight into the window in one pass when no upscaler runs, saving a full-size copy per picture.
