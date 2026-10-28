## en
- Snapdragon decoders get extra low-latency options when they support them.
- MediaTek decoders ask for shorter queues first and fall back to the longer ones if that setup fails.
- HEVC on newer Amlogic TV boxes (not Fire TV) no longer uses the low-latency decoder flags.
- **HEVC low-latency flags** is now Auto, On or Off. Auto leaves them out on newer Amlogic boxes only, On uses them there too, and Off leaves them out everywhere, as the older switch did.
- The debug overlay shows frame pacing: average interval, jitter, judder, how many vsyncs each frame took and a timeline of the last frames.
- A frame too large for the decoder's buffer restarts the decoder with a bigger one instead of ending the stream.
- A controller gyroscope that only reports zeros switches player 1 to the device's motion sensors.
