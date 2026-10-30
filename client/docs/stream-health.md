# Stream health

[Documentation](index.md)

**Why did my stream stutter?** explains stutter from measurements Nightjar already collects. Open the stream menu, then **Display → Overlay and diagnostics → Why did my stream stutter?**

The report uses up to 30 seconds of stream measurements, the start times of network stalls from the last 20 minutes, and the current Wi-Fi reading. It does not move the mouse, record video, contact any service, or write a history to disk. Copying the report puts only the displayed evidence and advice on the clipboard.

The report identifies network loss and variation, slow decoding, host processing, display waits and Android thermal pressure from repeated observations; several findings can appear together. Host processing covers more than encoding. Display waits are compared with the frames actually released rather than the requested frame rate, so a still or variable-refresh desktop does not count as dropped frames. Missing counters stay unavailable, and an empty report does not prove that every stage is healthy. The stage timings cannot be added up to an input-to-screen latency.

Play for a few seconds, then open the report right after a stutter. Sampling continues while menus are open and pauses while the app is in the background or a host action runs. Stage counters older than two seconds are not used, old history expires, and advice is withheld when there is no recent video.

When the network findings repeat and the host supports bitrate changes during a stream (Rubylight 3.0), the report offers **Try a lower bitrate**. It lowers the encoder bitrate by 20%, to no less than 500 kbps, for this session. Play similar content for another 30 seconds, then return and choose **Undo bitrate trial** if nothing improved. The trial is not offered while **Device controls bitrate**, automatic quality or a manual change controls the bitrate. With other hosts the report gives the same findings and advice without the trial.

## Wi-Fi facts

The report ends with a **Connection** section, and the **Debug** performance overlay lists the same lines under Network:

- **Wi-Fi band**, channel and channel width. The width is what the access point announces about its channel (Android 12 and later); Android offers no width field for the connection itself, so older versions show *not available*.
- **Wi-Fi signal** in dBm with a grade: excellent from −55 dBm, good from −67 dBm, fair from −75 dBm, weak below that. A weak signal adds a hint to move closer to the router.
- **Wi-Fi link speed** for sending and receiving (Android 10 and later; older versions show one combined rate). Link speed is the radio's current rate, not the stream's throughput.
- **Wi-Fi standard** (Wi-Fi 4 to 7, Android 11 and later).
- **Low-latency Wi-Fi lock**: whether Nightjar holds Android's low-latency Wi-Fi lock for this stream. The lock exists on Android 10 and later.
- A **2.4 GHz warning**: that band is slower and more crowded than 5 or 6 GHz and a common cause of stutter.

Ethernet, mobile data and VPN connections say so instead. Behind a VPN such as Tailscale, Nightjar shows the Wi-Fi facts when the device has a Wi-Fi association. Anything Android does not report reads *not available* rather than disappearing.

Nightjar reads these values every five seconds on a low-priority thread while a stream is connected; the overlay and the report use the latest reading, so the video, audio and input paths are unaffected. Only the network and Wi-Fi state permissions are needed. The network name (SSID) and the access point's address are not shown: Android treats them as location data, and Nightjar does not request location permission for this.

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

[Documentation](index.md) · [Video and latency](video-and-latency.md) · [Troubleshooting](TROUBLESHOOTING.md)
