# Video and latency

[Documentation](index.md) · [Settings reference](settings.md)

Use 720p/60 as a troubleshooting baseline, watch the overlay, then change one setting at a time. Resolution affects detail and decode work; frame rate affects motion and work per second; bitrate affects compression quality and network load.

## Codecs

| Choice | Use and requirements |
| --- | --- |
| Automatic | Negotiates a compatible hardware codec. Low-latency hardware AV1 is preferred when eligible, followed by HEVC/H.264; host capabilities, HDR and decoder checks affect the final choice. |
| H.264 | Useful for compatibility and as a baseline when another decoder stalls. |
| HEVC | Useful for high-quality streams at a lower bandwidth budget and compatible Main10 HDR decoding. |
| AV1 | Uses compatible hardware and host encoding. Compare actual decode time on your device. |
| PyroWave | Vulkan codec for the lowest decode time at high bitrates, on hosts that support it (currently Rubylight). Choose it explicitly on a 64-bit Android/Vulkan device. |

PyroWave requires Android 10+, Vulkan 1.3 and the GPU features, limits and presentation formats checked by the renderer. Start with about 280 Mbps for 720p/60 or 400 Mbps for 1080p/60 on a fast LAN. Tune from there with the overlay. Nightjar requests up to 2000 Mbps at startup and during a stream; the host applies its own ceiling. Nightjar switches to a conventional codec automatically when a device needs it.

A PyroWave host sends each frame as one burst at its link speed. Some receive paths, such as USB Ethernet docks, lose the end of such bursts below the link's rated speed. On a host with frame-shaped UDP tests (Vibeshine), **PC settings → Calibrate frame pacing** finds the fastest burst speed this device takes without losing frame tails. It saves that speed for the PC and sends it with each PyroWave stream as the host's frame pace. Stop any stream on the PC first; the test takes one to three minutes. The pace belongs to the connection it was measured on, so it is only sent while this device's wired link speed toward the host is the same as during the test.

Long-press a PC and use **PC settings… → Connection test** before increasing bandwidth. On hosts that support it (currently Rubylight), its optional, cancellable 32 MiB HTTPS download starts only when requested and reports throughput and the host link speed.

## HDR and color

HDR10 needs an HDR10 display, Android 7+ and a compatible HEVC Main10 or AV1 10-bit decoder, plus HDR configured on the streamed host display. PyroWave has its own HDR-capability checks. Enable **HDR** in Stream settings or the PC profile.

Nightjar handles BT.2020/PQ and static mastering metadata. Debug stats show the negotiated format, so you can confirm HDR is active.

**Prefer YUV 4:4:4** can improve colored text. It applies when the host and decoder both support it, with 4:2:0 used otherwise. HDR takes priority over a conflicting chroma preference. **Force full-range video** is an explicit override: mismatched range can crush blacks or clip highlights.

## Refresh rate and VRR

With VRR off, **Frame pacing** balances immediate response against regular presentation. **Lowest latency** favors fresh output; **Balanced** aligns cadence; **Balanced with FPS limit** and **Smoothest video** retain more queued output.

With **Variable refresh (VRR)** on, Nightjar requests the screen's maximum refresh and shows decoded frames at the game's cadence. Hosts that support it (e.g. Rubylight or Vibepollo) are also asked for host-side VRR. Confirmed Android 16 adaptive refresh uses cadence votes; other screens use a maximum-refresh fallback. VRR forces Lowest latency pacing and disables Allow lower refresh rate.

Per-PC profiles accept rates to 0.01 Hz, such as 59.94 or 119.88. Hosts that support fractional rates (currently Rubylight) receive them as set; other hosts receive a whole-number rate. The overlay reports received, released and shown FPS separately from panel refresh.

## Video scaling

**Settings → Display & image → Video scaling** decides how the stream fills a screen of another shape. The stream menu's **Video scaling** changes it during a stream when no upscaler or PyroWave is active; with either of those it applies to the next stream.

| Mode | Result |
| --- | --- |
| Fit (default) | The whole picture, with bars on the sides or above and below. |
| Fill (crop) | Fills the screen without distortion; the edges that do not fit are cut off evenly. |
| Stretch | Fills the screen; shapes look wider or taller. |
| 1:1 pixels | One stream pixel per screen pixel, centred. A smaller stream gets a border; a larger one is cropped. |

**Video position** (Centre or Top) places a Fit or 1:1 picture that leaves room: Top keeps the space below free, for the soft keyboard or the lower half of a half-folded phone. Fill and Stretch cover the whole screen, so the position changes nothing there. Fill and Stretch also use the area beside a camera cutout, like a stream at the panel's own size; Fit and 1:1 keep to the cutout-safe area. Picture-in-picture always shows the whole stream (Fit, or Stretch if chosen).

Scaling is a view transform only: the decoder, its buffers and the frame path are unchanged, and touch, mouse, pinch zoom and the Local cursor map through it (see [touch mapping](controls.md#touch-mapping-and-video-scaling)). With an upscaler, Fill draws the cropped edges too, so it costs a little more GPU time than Fit; 1:1 shows the stream at its own size, so there is nothing to upscale.

Older versions only know **Stretch to fill screen** (`checkbox_stretch_video`). Nightjar keeps writing that switch (on only for Stretch), so after a downgrade Fill and 1:1 show as Fit, and a change made in an older version wins after the next upgrade.

## Orientation

**Settings → Display & image → Orientation** decides which way the stream screen may turn. It is off by default: **Automatic** keeps the earlier behaviour, with phones in landscape and tablets, squarish screens, desktop-class devices, split-screen or desktop windows and external displays free to turn.

| Choice | Screen | Stream shape |
| --- | --- | --- |
| Automatic (default) | Phones in landscape; the others turn freely. | Unchanged. |
| Landscape | Locked to landscape, either way up. | Unchanged. |
| Portrait | Locked to portrait, either way up. | Portrait from the start on a host streaming from its virtual display. |
| Rotate with device | Follows the device, unless the system rotation lock is on. | Starts in the screen's shape on a host streaming from its virtual display, and turns with the device where that host also resizes a running stream. |

Only a host that draws the stream on its own virtual display can give it a portrait shape (Apollo, Rubylight and Vibepollo with the virtual display on; see [host support](PARITY-MATRIX.md)). With **Rotate with device**, Nightjar then asks the host for the turned size during the stream, through the same in-place resize used for foldables (control message `0x5532`). A turn waits one second for the device to settle, so turning the phone over and straight back sends nothing. As for foldables, the decoder is readied for the new size before the host is asked, and nothing waits on the video path. A fixed resolution is turned as it is (1920×1080 becomes 1080×1920); a native one follows the panel. Sizes are rounded down to even numbers and to the decoder's alignment, and shrunk with their shape kept if the decoder cannot take the turned size at full height.

Every other host, and a host that cannot resize a running stream, keeps the stream's size: the picture is placed in the turned screen by [Video scaling](#video-scaling), with bars or cropping as chosen, and Nightjar says so once per stream. The same notice appears if the device's decoder cannot switch to the turned size. A stream on an external display ignores the phone's orientation, and the setting is hidden on TVs.

Touch, mouse, pinch zoom, the Local cursor and on-screen controls map through the laid-out picture, so they stay aligned after a turn. The on-screen controls are laid out again for the new screen shape; their layout was designed for landscape and is roomier there.

## Upscaling

Choose **Settings → Display & image → Client-side upscaling** or override it in a PC profile. First lower **Stream resolution**, for example to 720p on a 1080p screen. A PC profile also accepts a custom stream resolution. This reduces transmitted pixels and decode work; upscaling reconstructs that smaller video at the Android output size. **Host render scale** changes the PC's render resolution, not the transmitted video size.

| Mode | Processing |
| --- | --- |
| Off (default) | Direct output without FSR/SGSR reconstruction. |
| Bilinear | Bilinear scaling through GLES for MediaCodec or Vulkan for PyroWave. |
| FSR 1.0 | AMD EASU reconstruction followed by RCAS sharpening. |
| SGSR 1 | Qualcomm's single-pass spatial reconstruction. |

MediaCodec GPU upscaling requires lower-resolution SDR video, GLES 3.0 and the required external-image support. Native/larger input, HDR and 10-bit use direct output. Under GPU overload or after a resize, this path switches to direct output for the rest of the session; reconnect to retry.

PyroWave SDR streams use SGSR 1 inside the Vulkan renderer for both FSR and SGSR, reading the decoded planes directly in one pass. The output follows the actual Android surface size independently of the decoded stream size. It preserves the video's aspect ratio unless **Video scaling** is Stretch. Upscaling needs an output viewport larger than the source; HDR/10-bit and a failed SGSR pipeline keep direct output. The Debug overlay shows the active path or the reason it is inactive.

Sharpening ranges from 0–100%, default 50%. At 0%, FSR bypasses extra sharpening; SGSR still performs spatial reconstruction. The setting applies next stream. Compare text, edges, motion, battery use and added time, not just a still image.

## Snapdragon GPU drivers

**Settings → Advanced → Snapdragon GPU driver** selects the Vulkan driver used by PyroWave; it is shown on arm64 Adreno devices only. **System GPU driver** is the default. Custom drivers require Android 10+, an arm64 app and an Adreno GPU; they do not replace Android's system driver or the MediaCodec drivers used by H.264, HEVC and AV1. The selected driver must still pass PyroWave's Vulkan checks.

Use **Import driver ZIP** to import and select a trusted Adreno package, or choose an already imported driver. Import and selection refresh the capability check; the next stream uses the selection. **Remove a driver** deletes its private app copy and returns to the system selection if that driver was selected. Stop a stream before removing its active driver. Imported drivers are excluded from app backups.

Packages need a root-level `meta.json` with schema version 1, a compatible `minApi`, name, author, package/driver versions, Mesa or Qualcomm vendor, and the named arm64 `.so` library. The importer rejects folders, unsafe or duplicate paths, non-arm64 libraries and oversized packages. Limits are 128 MiB compressed, 256 MiB expanded, 64 files, 16 KiB metadata and eight installed drivers. Package validation does not establish compatibility with a particular GPU.

If loading, capability checks or initialization fail, Nightjar tries the system driver and reports the fallback. A failed custom selection remains available to retry or remove. If the app exits while a custom driver is active, the next launch returns that selection to the system driver. Loaded driver libraries and capability results remain in the process until it exits; switching drivers affects a new stream without unloading libraries from an active one. To retry a failed load of the same installed package, fully close and reopen the app, then select it again. The Debug overlay includes the driver used by that stream. Performance and stability need testing on the actual phone; custom drivers are optional, not a guaranteed speed improvement.

## Performance overlay

Show or hide the overlay with **Overlay** in the stream menu's top row or **Ctrl+Alt+Shift+S**, and choose **Compact**, **Statistics** or **Debug** under **stream menu → Display → Overlay and diagnostics**. The saved default is under **Settings → Streaming → Performance overlay mode**, which offers Debug while Developer options are shown. **Copy stats** copies the full Debug text regardless of visible mode. Both modes hide in picture in picture.

Compact is the default: one line such as `● 119.9 FPS · 8 ms RTT · 0.1% loss`, with displayed FPS, the measured network round trip (labelled RTT, not end-to-end latency) and frame loss. The dot is green, turns amber from 1% loss, a 40 ms round trip, a tenth of the received frames not shown or decoding slower than a frame, and red from 5% loss, an 80 ms round trip or a quarter of the frames not shown. Compact sits top-start, moves to a free corner or the top centre when on-screen controls would sit under it, and keeps clear of camera cutouts. Statistics shows stream resolution/codec/color, displayed and received FPS, received video payload Mbps, network frame loss, network RTT and variation, host processing, decode time and display wait. These are separate measurements: adding network RTT and decode time does not measure end-to-end latency. Payload Mbps excludes audio, protocol overhead and retransmissions, so it is not the configured bitrate or total link traffic.

Unavailable measurements display **—**, not zero. Some devices or rendering paths cannot report display callbacks, and host processing requires host-provided data. **Measure input latency** runs a separate input-to-rendered-frame probe; neither overlay mode infers that measurement from the displayed stages. Optional [host hardware metrics](#host-hardware-metrics) appear separately from these timings.

Debug groups diagnostics into Video, Network and Decode and scrolls on smaller screens. It adds timing percentiles, decoder settings, GPU driver and upscaling details where available. Existing Advanced selections become Debug. Statistics ends with a status line of lasting facts about the stream, such as HDR or a codec this PC or device lacks; each is also said once per PC as a message.

| Debug metric | Meaning and interpretation |
| --- | --- |
| Resolution, codec, HDR/chroma, decoder | Actual negotiated stream and selected decoder; compare with requested settings. |
| Received / released / shown FPS | Frames received, outputs released and presentation callbacks observed. Low shown FPS with healthy receive/decode rates points toward presentation. |
| Estimated display drops / present drops | Released frames without an observed presentation callback after five seconds. |
| Network latency / variation | Control-channel round-trip time and its variation in milliseconds. |
| Network frame loss | Frames lost through the transport/recovery path, expressed as a percentage; separate from local decoder/display drops. |
| Queue wait | Enqueue-to-decoder-input time. High values suggest submission or input-buffer backlog. |
| Decode time | Each frame passed on for display is timed from input submission to its hand-off, including driver scheduling, in whole milliseconds. The total over the last two seconds is divided by all frames received in that time; the figure after streaming does the same over the whole stream. It excludes pre-input queue wait and later display wait. The p95/p99 beside it, and the frame-timing CSV, use exact per-frame times from input submission to decoder output. |
| Display wait | Decoder output to a rendered callback. |
| Client latency | The client pipeline interval reported by the renderer. |
| avg / p95 / p99 | Mean and percentile timings over valid samples; the Decode time average is the one described above. MediaCodec stage statistics retain up to 600 frames; each stage has its own valid sample count. |
| Host processing | Host-reported processing avg/p95/p99. Compare host load separately from Android decode work. |
| Low-latency support / accepted settings | Advertised decoder capabilities and echoed input-format keys. Compare decode time to see their effect. |
| Upscaling mode / added time / fallback | Active GPU path or reason for direct output. MediaCodec reports an added-time estimate including texture handoff, GPU completion and swap wait; PyroWave reports the path without a separate upscaling-time estimate. |
| PyroWave queue / completed decode / GPU decode | Queued work, completed frame decode and the last GPU decode measurement, each reported separately. |
| PyroWave missing records | Lost portions of record-framed data. Received records update the picture right away while lost regions keep their previous detail. |
| PyroWave waits / replaced frames | PyroWave decodes into three plane sets on the stream thread while a separate present thread waits for the display, so decoding never queues behind presentation. "Free planes" is the decoder's wait for a plane set, "screen image" the present thread's wait for a swapchain image, and "replaced" the share of decoded frames a newer one superseded before display. |
| Wi-Fi band / signal / link speed / standard | The active Wi-Fi link: band, channel and announced width, RSSI with a grade, send/receive link rate, Wi-Fi generation and whether the low-latency Wi-Fi lock is held. A 2.4 GHz link or weak signal adds a warning; Ethernet, mobile data and VPN say so. See [Stream health](stream-health.md#wi-fi-facts). |
| Stall rhythm | Shown when recent network stalls repeat on a steady beat, the pattern of background Wi-Fi scans. See [Stream health](stream-health.md#stalls-every-few-minutes). |
| Bitrate / automatic bitrate status | Requested or host-applied bitrate and adaptation state, subject to host caps. |

## Host hardware metrics

This needs a host that supports it (currently Rubylight). On the host, enable **Send hardware metrics to client** (on Rubylight: **Settings → General → Hardware metrics**). It is off by default. Start or reconnect a video stream and show the **Statistics** or **Debug** overlay. The client checks the host's advertised protocol support; the host version number alone does not establish compatibility. Normal streams and Remote Monitor qualify, but input-only sessions do not. Pairing and **View streams** or **Launch applications** permission are required, and the active video stream must belong to this device.

Statistics adds host CPU usage and each reported GPU's usage, temperature and GPU power. Debug adds a **Host hardware** section with GPU names/IDs, available clocks, hotspot/memory temperatures, voltage, fans, GPU/board power, VRAM readings and sample details. GPU readings need an AMD driver with ADLX support; Windows CPU usage can be available without an AMD GPU. CPU temperature and CPU clocks are not provided. These are host-wide readings, not per-game measurements; a listed GPU is not necessarily the capture or encoding GPU.

The host's **Hardware metrics selection** controls which readings it sends; an empty selection requests all supported readings. Each reading is optional, and missing values appear as **—** rather than zero. A present zero, such as a stopped fan, is valid. The first CPU sample can be unavailable. Each new sample replaces the previous readings, including any GPUs that are now absent. Failed or stale samples remove the extra hardware section; a valid empty sample shows CPU usage as unavailable. Stream-provided **Host processing** time remains independent of this feature.

Requests use the paired streaming HTTPS connection, independently of video decoding. Android polls at most once every two seconds, or less often when the host's sampling interval is longer. Polling stops when the overlay is hidden, the app is paused or in picture in picture, or the stream ends. Temporary failures clear the values and retry with backoff; denied access, an unavailable endpoint or an unsupported schema stop telemetry for that session. After enabling the host setting or restoring permissions, reconnect to rediscover support. **Copy stats** includes available hardware readings and GPU names. See the host's [hardware metrics protocol](https://github.com/RamazanKara/Rubylight/blob/main/docs/hardware-metrics.md) for field definitions and units.

## Network drops

When the network goes silent for a moment (a phone roaming between Wi-Fi access points, a short dropout, a VPN such as Tailscale moving between Wi-Fi and mobile data), the stream does not end straight away. Sends that find no route are treated as lost packets and resent. What ends the stream is the control connection's timeout: by default, 10 seconds without hearing from the host.

**Settings → Streaming → Ride out network drops** raises that timeout to 20, 30 or 60 seconds. It is off by default, because a longer timeout also keeps a stream that is really gone on screen for longer. While the network is silent the picture freezes. Once nothing has come back from the host for a second, Nightjar stops sending input instead of queueing it for later, so a long silence does not end in seconds of old input arriving at once. When the host answers again, Nightjar sends where things stand now: keys and buttons let go meanwhile are released, ones still held are pressed, controllers send their current state, and touches are cancelled. Mouse movement, scrolling and text from the silence are dropped, and input from its first second can still arrive late. The next video frames resume the picture.

The host has its own timeout for a silent client, usually also 10 seconds, and it ends the session on its side when that runs out. A wait longer than 10 seconds therefore only helps if the host is set to wait as long. Sunshine and its forks (Apollo, Vibepollo) call this `ping_timeout` in the host configuration, in milliseconds; NVIDIA GameStream has no such setting.

If the network does not come back in time, the stream ends with an error that says the drop tolerance ran out (or that the host may have ended the session first). With [**Recover interrupted streams**](recovery-and-quality.md#recovering-a-connection) also on, Nightjar then reconnects to the same running game within its usual one-minute budget.

The setting covers the network path staying the same, which is the case for roaming within one Wi-Fi network and for a VPN that keeps its address. A change of the device's address that the host can see, such as leaving Wi-Fi for mobile data without a VPN, needs the host to accept the client from its new address; that is a [proposal for hosts](protocol/client-roaming.md), not something Nightjar can do alone.

## Automatic bitrate and diagnostics

For bounded reconnects and opt-in resolution/FPS adaptation, see [stream recovery and automatic quality](recovery-and-quality.md).

Enable **Automatic bitrate** in the stream menu's **Quality** section on a host that supports it (currently Rubylight). It is opt-in and saved per PC, separately for conventional codecs and PyroWave. Loss, RTT variation and completed decode headroom inform adaptation; PyroWave also considers queue delay. Bitrate recovers gradually once the link is healthy. Manual session bitrate changes turn it off.

Use **Settings → Developer options → Export latency CSV** to save the latest local frame timings. `queue_wait_ms`, `input_to_output_ms` and `output_to_render_ms` correspond to the stages above; The appended FPS sample separates received/released/shown FPS and estimated presentation drops.

**Stream menu → Display → Overlay and diagnostics → Measure input latency** runs a software response probe during a stream: on a still host desktop with a visible pointer, it nudges the mouse 15 times and times each nudge until a subsequent frame receives a rendered callback. It reports typical, best and slowest-10% values. Keep the host desktop still between nudges so unrelated updates do not affect the result. This interval ends at the callback; it does not measure physical input-device delay or the screen's actual light output.

**Settings → Developer options → Latency benchmark** runs a 10-second hostless test of frame callbacks, timer wakeups and input dispatch age. Use it to compare devices and settings without a host; **Measure input latency** exercises the stream's software response path. **Share results** on the same screen turns this test and the decoder benchmark into redacted text you can copy or share yourself; see [device results](devices.md).
