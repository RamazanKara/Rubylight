## en
- PyroWave pictures that arrive early reach the decoder without two extra copies through Java.
- The connection thread wakes only when the connection needs it, instead of 200 times a second, which saves battery during a stream.
- Gamepad input, the latency log and the on-screen sticks allocate less while streaming, and phone gyro aiming no longer asks the system for the screen rotation on every sample.
- The GPU upscaler and full-colour mode wait for the GPU directly instead of checking every millisecond.
- Video arrives in fewer, larger reads where the system can combine packets (also on Linux desktops).

## desktop
- Windows: PyroWave and full-colour (4:4:4) pictures are drawn straight to the window instead of through an extra full-size image. On a Radeon 780M laptop at 2880x1800 and 120 fps, presenting a picture took 0.07 ms of CPU instead of 0.19 ms, and decode to present fell from 0.44-0.46 to 0.39 ms at the median and 0.55-0.72 to 0.50 ms at p95.
- Linux: VA-API pictures are drawn straight from the decoder's memory instead of being copied first. On an Intel NUC (Iris 655) at 3840x2160, 60 fps, HEVC 10-bit: decode to present fell from 1.95-1.98 to 1.60-1.63 ms at the median, and present to screen from 1.66 to 1.08 ms.
- The stats overlay is laid out only when its text changes instead of on every picture, and an HDR desktop on Linux no longer asks the GPU driver about its HDR support several times per picture.
