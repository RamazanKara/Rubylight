## desktop
- Linux: Nightjar asks AMD's Mesa drivers for their low-latency video decode mode (lowlatencydec), keeping your own AMD_DEBUG and RADV_PERFTEST flags. Programs Nightjar starts get your original values.
- Linux: with V-Sync off, pictures go to the display at once, as on Windows, instead of waiting in the compositor's queue. A Variable refresh rate stream on Wayland stays tear-free while the display actually runs VRR, and tears like any other stream on a display that does not. The log names the presentation mode the display uses.
- Linux: on Wayland, the log now records when each picture reached the screen and ends each stream with a summary, and the timing CSV gains the matching columns.
- Linux: Vulkan Video, PyroWave and 10-bit or full-chroma software-decoded pictures are converted and scaled straight into the window in one pass when no upscaler runs, saving a full-size copy per picture.
