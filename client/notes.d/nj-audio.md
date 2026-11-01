## en
- Android: plugging in headphones, connecting Bluetooth or a USB DAC mid-stream reopens the low-latency audio output on the new device, instead of leaving it for the slower fallback output (with its higher latency and no gap concealment) for the rest of the stream.
- Android and desktop: when the device's audio clock runs a little slower than the PC's, the client now trims the extra audio one packet at a time with a short fade. Before, audio latency crept up by up to 40 ms over about 20 minutes, then a packet was cut with a click.
