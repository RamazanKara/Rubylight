# Stream health

Open the stream menu, then **Display → Overlay and diagnostics → Why did my stream stutter?** The report uses up to 30 seconds of measurements already collected by the client, plus the start times of network stalls from the last 20 minutes and the current Wi-Fi reading. It does not run the mouse-moving latency test, record video, contact an analytics service, or write a history to disk. Copying the report puts only its displayed evidence and advice on your clipboard.

Repeated observations can identify network loss/variation, slow decode output, host processing, display waits, and Android thermal pressure. Several findings may appear together. Host processing includes more than encoding. Display waits are compared with observed frame release rather than the requested frame rate, so a static or variable-refresh desktop does not automatically count as dropped frames. Missing counters stay unavailable; a lack of findings does not prove every stage is healthy. These measurements cannot be added together to claim input-to-screen latency.

Collect a few seconds of gameplay, then open the report just after a stutter. Sampling continues while navigating menus and pauses during backgrounding and host actions. Individual stage counters must be no more than two seconds old before entering the history. Old history expires, and advice is withheld when recent video is unavailable.

For repeated network evidence, a host that supports runtime bitrate changes (Rubylight) can offer **Try a lower bitrate**. It reduces the current encoder bitrate by 20%, with a 500 Kbps floor, for this session. Compare similar gameplay for another 30 seconds, then return and choose **Undo bitrate trial**. Device controls bitrate, automatic quality and manual changes take ownership of the settings; the doctor does not compete with them. Other hosts still get observations and manual advice when their counters are available; unsupported runtime changes are not offered.

## Wi-Fi facts

The report ends with a **Connection** section, and the **Debug** performance overlay lists the same lines under Network:

- **Wi-Fi band**, channel and channel width. The width is what the access point announces about its channel (Android 12 and later); Android offers no width field for the connection itself, so older versions show *not available*.
- **Wi-Fi signal** in dBm with a grade: excellent from −55 dBm, good from −67 dBm, fair from −75 dBm, weak below that. A weak signal adds a hint to move closer to the router.
- **Wi-Fi link speed** for sending and receiving (Android 10 and later; older versions show one combined rate). Link speed is the radio's current rate, not the stream's throughput.
- **Wi-Fi standard** (Wi-Fi 4 to 7, Android 11 and later).
- **Low-latency Wi-Fi lock**: whether Nightjar holds Android's low-latency Wi-Fi lock for this stream. The lock exists on Android 10 and later.
- A **2.4 GHz warning**: that band is slower and more crowded than 5 or 6 GHz and a common cause of stutter.

Ethernet, mobile data and VPN connections say so instead. Behind a VPN such as Tailscale, Nightjar shows the Wi-Fi facts when the device has a Wi-Fi association. Anything Android does not report reads *not available* rather than disappearing.

Nightjar reads these values every five seconds on a separate low-priority thread while a stream is connected. The overlay and report only read the last reading, so nothing is added to the video, audio or input paths. It needs only the network and Wi-Fi state permissions Nightjar already has. **The network name (SSID) and access point address are not shown**: Android treats them as location data and releases them only with location permission, which Nightjar does not request for this.

## Stalls every few minutes

A "lag spike every two minutes" is often a background Wi-Fi scan: the radio leaves the channel for a moment to look for other networks. Nightjar keeps the start time of each network stall (frame loss of at least 2% or round-trip variation above a frame interval) for 20 minutes. When at least four stalls fall on a steady beat between 30 seconds and 10 minutes apart, allowing one missed beat and a few unrelated stalls, the report adds **Repeating stalls** with the period, and the Debug overlay shows **Stall rhythm**. A rhythm needs a few minutes of streaming to show, so it appears even when the last 30 seconds were clean.

To reduce scans:

- On Android, open **Settings → Location → Location services** and turn off **Wi-Fi scanning** and **Bluetooth scanning**. The names and path vary by device.
- Leave **Developer options → Wi-Fi scan throttling** on, so apps cannot scan often.
- Close apps that look for networks or nearby devices while you stream.
- On the PC, use a cable, or stop tools that scan for Wi-Fi networks.

Nightjar already holds Android's low-latency Wi-Fi lock while streaming, which asks the system to defer scans and power saving. Some devices scan anyway, which is why the rhythm check exists.

## Frames in bursts

Some Wi-Fi links hold video back for a few tens of milliseconds and then deliver several frames at once, although the host sent every frame on time. The picture then hitches once per burst. Nightjar compares each frame's arrival with the host's timestamps: a frame that comes at least three quarters of a frame interval (8 ms at the least) later than its timestamp says, after the frame before it, is late. Frames the host itself sent late, such as a still desktop, a game that slows down or a frame that took the host longer to encode than the one before, do not count. A sample with late frames is a vote; a run of up to 10 such samples in a row counts as 2 votes, because a launch or a switch to fullscreen holds frames back for a few seconds once and is not reported. When the votes reach 6 and a third of the samples, the report adds **Bursts**.

Thresholds are conservative troubleshooting hints, not a measured improvement guarantee. Live stream, decoder and device testing is still needed to compare actual latency and picture quality.

[Documentation](index.md) · [Video and latency](video-and-latency.md) · [Troubleshooting](TROUBLESHOOTING.md)
