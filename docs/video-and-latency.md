# Video and latency

[Documentation](index.md) · [Settings reference](settings.md)

Start with the default 720p/60 stream, watch the overlay, then change one setting at a time. Resolution affects detail and decode work; frame rate affects motion and work per second; bitrate affects compression quality and network load.

## Codecs

| Choice | Use and requirements |
| --- | --- |
| Automatic | Negotiates a compatible hardware codec. Low-latency hardware AV1 is preferred when eligible, followed by HEVC/H.264; host capabilities, HDR and decoder checks affect the final choice. |
| H.264 | Useful for compatibility and as a baseline when another decoder stalls. |
| HEVC | Useful for high-quality streams at a lower bandwidth budget and compatible Main10 HDR decoding. |
| AV1 | Uses compatible hardware and host encoding. Compare actual decode time on your device. |
| PyroWave | Explicit experimental Vulkan choice; Automatic does not select it. Requires a compatible Rubylight host and a supported 64-bit Android/Vulkan device. |

PyroWave requires Android 10+, Vulkan 1.3 and the GPU features, limits and presentation formats checked by the renderer. Start with about 280 Mbps for 720p/60 or 400 Mbps for 1080p/60 on a fast LAN. These are starting budgets, not measured throughput guarantees. A PC profile accepts startup requests up to 2000 Mbps; runtime bitrate changes are capped at 500 Mbps. Incompatible devices or bitstreams use a conventional codec fallback.

Long-press a PC and use **This PC → Test your connection** before increasing bandwidth. Its optional, cancellable 32 MiB HTTPS download starts only when requested and reports throughput and the host link speed, not video decode speed or UDP packet loss.

## HDR and color

HDR10 needs an HDR10 display, Android 7+ and a compatible HEVC Main10 or AV1 10-bit decoder, plus HDR configured on the streamed host display. PyroWave has its own HDR-capability checks. Enable **HDR** in Stream settings or the PC profile.

Rubylight handles BT.2020/PQ and static mastering metadata. Dynamic HDR10+ metadata is not supplied by this path. Check the negotiated format in Advanced stats; an HDR request does not prove that HDR was selected.

**Prefer YUV 4:4:4** can improve colored text. Both host and decoder must support the requested chroma format; otherwise the client uses 4:2:0. HDR takes priority over a conflicting chroma preference. **Force full-range video** is an explicit override: mismatched range can crush blacks or clip highlights.

## Refresh rate and VRR

With VRR off, **Frame pacing** balances immediate response against regular presentation. **Lowest latency** favors fresh output; **Balanced** aligns cadence; **Balanced with FPS limit** and **Smoothest video** retain more queued output.

With **Variable refresh (VRR)** on, Rubylight requests the screen's maximum refresh and shows decoded frames at the game's cadence. Confirmed Android 16 adaptive refresh uses cadence votes; other screens use a maximum-refresh fallback. VRR forces Lowest latency pacing and disables Allow lower refresh rate. OS power saving, thermal limits and display policy can constrain the actual rate.

Per-PC profiles accept rates to 0.01 Hz, such as 59.94 or 119.88. Displayed FPS, panel refresh and received FPS are different measurements; a 120 Hz display mode does not establish that 120 distinct frames were shown.

## Upscaling

Choose **Settings → Stream → Client-side upscaling** or override it in a PC profile.

| Mode | Processing |
| --- | --- |
| Off (default) | Android's direct compositor scaling. |
| Bilinear | GLES bilinear scaling. |
| FSR 1.0 | AMD EASU reconstruction followed by RCAS sharpening. |
| SGSR 1 | Qualcomm's single-pass spatial reconstruction. |

GPU upscaling is for lower-resolution MediaCodec SDR video on GLES 3.0 with the required external-image support. Native/larger input, HDR, 10-bit and PyroWave use direct output. GPU overload, repeated local drops, renderer errors or resize can cause direct-output fallback until reconnect.

Sharpening ranges from 0–100%, default 50%. At 0%, FSR bypasses extra sharpening; SGSR still performs spatial reconstruction. The setting applies next stream. Compare text, edges, motion, battery use and added time, not just a still image.

## Performance overlay

Use **stream menu → Overlay** or **Ctrl+Alt+Shift+S**. Compact is the default mode; hold the overlay to switch to Advanced. **Copy stats** copies all Advanced measurements regardless of visible mode. Both modes hide in picture in picture.

Compact displays shown FPS, network RTT plus completed decode time in milliseconds, and network frame loss. Missing measurements are omitted. Its latency value is a diagnostic sum, not input-to-photon latency.

The health dot is green below 30 ms and 1% loss, amber at either threshold or when measurements are incomplete, and red at 60 ms or 5% loss.

Compact includes PyroWave measurements. Advanced groups details into Video, Network, Decode and Host and scrolls on smaller screens. The selected mode persists between streams.

| Advanced metric | Meaning and interpretation |
| --- | --- |
| Resolution, codec, HDR/chroma, decoder | Actual negotiated stream and selected decoder; compare with requested settings. |
| Received / released / shown FPS | Frames received, outputs released and presentation callbacks observed. Low shown FPS with healthy receive/decode rates points toward presentation. |
| Estimated display drops / present drops | Released frames without an observed callback after five seconds. Delayed or absent callbacks can affect the estimate; it is not a physical scanout count. |
| Network latency / variation | Control-channel RTT and variation in milliseconds. They are not one-way latency. |
| Network frame loss | Frames lost through the transport/recovery path, expressed as a percentage; separate from local decoder/display drops. |
| Queue wait | Enqueue-to-decoder-input time. High values suggest submission or input-buffer backlog. |
| Decode time | Input submission to observed decoder output, including driver scheduling. It excludes pre-input queue wait and later display wait. Observed outputs count even if dropped before presentation. |
| Display wait | Decoder output to a rendered callback. Missing callbacks leave this measurement unavailable. |
| Client latency | The client pipeline interval reported by the renderer; it does not include every host, input and physical display stage. |
| avg / p95 / p99 | Mean and percentile timings over valid samples. MediaCodec stage statistics retain up to 600 frames; each stage has its own valid sample count. |
| Host processing | Host-reported processing avg/p95/p99. Compare host load separately from Android decode work. |
| Low-latency support / accepted settings | Advertised decoder capabilities and echoed input-format keys. Accepted keys do not prove a latency improvement. |
| Upscaling mode / added time / fallback | Selected GPU path, its added-time estimate and a reason for direct output. GPU texture callbacks are not display presentation measurements. |
| PyroWave queue / completed decode / GPU decode | Queued work, completed frame decode and the last GPU decode measurement are distinct; do not substitute one for another. |
| PyroWave missing records | Missing portions of record-framed data. Surviving records can update while missing regions reuse earlier coefficients. |
| Bitrate / automatic bitrate status | Requested or host-applied bitrate and adaptation state, subject to host caps. |

## Automatic bitrate and diagnostics

Enable **Automatic bitrate** in the stream's **Bitrate** menu on a supporting host. It is opt-in and saved per PC, separately for conventional codecs and PyroWave. Loss, RTT variation and completed decode headroom inform adaptation; PyroWave also considers queue delay. Recovery is gradual and missing measurements prevent increases. Manual session bitrate changes turn it off.

Use **Settings → Advanced → Export latency CSV** to save the latest local frame timings. `queue_wait_ms`, `input_to_output_ms` and `output_to_render_ms` correspond to the stages above; unavailable measurements stay blank. The appended FPS sample separates received/released/shown FPS and estimated presentation drops. A full logging queue may omit rows.

**Settings → App → Latency benchmark** runs a 10-second hostless test of frame callbacks, timer wakeups and input dispatch age. It does not measure networking, decoding, physical controller polling, audio output, scanout or end-to-end stream latency.
