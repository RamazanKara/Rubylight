# Host hot path
- **Fewer Windows calls in the frame path.** The route to the client (network adapter and its speed) and the display's colour and HDR state are now checked on their own threads every two seconds and every second, as before, instead of on the encode thread before a frame goes to the encoder and on the video sender between frames. The capture thread no longer waits behind a display check either. The 5 s `stream timings` log line shows what is left (`stage_checks_ms`) and the slowest route lookup (`link_lookup_max_ms`).
- The route lookup skips the adapter's traffic counters, which it never used.
- `video_send_mmcss_only` (off by default) runs the video packetizer and sender with only the multimedia scheduler's boost, as the input thread does, for measurements under CPU load.
