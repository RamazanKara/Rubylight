## desktop
- Windows: with Variable refresh rate On, a frame that arrives while the screen is still drawing the one before waits until the screen has finished, as in 5.4.0. Nightjar 5.4.1 stopped waiting after 2 ms, so on 120 Hz screens such frames could tear (#111).
- The log's VRR lines at the end of a stream count the frames that went out before the screen had finished.
