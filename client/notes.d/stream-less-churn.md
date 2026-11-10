## en
- Audio packets no longer get a new buffer each on their way to playback, and the low-latency audio output no longer makes a temporary copy of each one.
- With "Video data straight to the decoder" off, the video buffer is replaced less often while frame sizes grow, which leaves less memory to clean up early in a stream.
- After handing a frame to the decoder, Nightjar no longer waits for the decoder's next free input buffer on the thread that receives the stream; the next frame waits for it instead, if it has to.
## desktop
- Each video frame goes to FFmpeg as it was received instead of being copied into a separate buffer first.
- On Linux, the end-of-stream log no longer says "0 of N presents drawn straight into the swapchain" for the OpenGL renderer, which never draws that way; it says the count does not apply there.
