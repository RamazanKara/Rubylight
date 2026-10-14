# Stream health

Open the stream menu, then **Display → Overlay and diagnostics → Why did my stream stutter?** The report uses up to 30 seconds of measurements already collected by the client. It does not run the mouse-moving latency test, record video, contact an analytics service, or write a history to disk. Copying the report puts only its displayed evidence and advice on your clipboard.

Repeated observations can identify network loss/variation, slow decode output, host processing, display waits, and Android thermal pressure. Several findings may appear together. Host processing includes more than encoding. Display waits are compared with observed frame release rather than the requested frame rate, so a static or variable-refresh desktop does not automatically count as dropped frames. Missing counters stay unavailable; a lack of findings does not prove every stage is healthy. These measurements cannot be added together to claim input-to-screen latency.

Collect a few seconds of gameplay, then open the report just after a stutter. Sampling continues while navigating menus and pauses during backgrounding and host actions. Individual stage counters must be no more than two seconds old before entering the history. Old history expires, and advice is withheld when recent video is unavailable.

For repeated network evidence, a host that supports runtime bitrate changes (currently Rubylight) can offer **Try a lower bitrate**. It reduces the current encoder bitrate by 20%, with a 500 Kbps floor, for this session. Compare similar gameplay for another 30 seconds, then return and choose **Undo bitrate trial**. Automatic bitrate/quality and manual changes take ownership of the settings; the doctor does not compete with them. Other hosts still get observations and manual advice when their counters are available; unsupported runtime changes are not offered.

Thresholds are conservative troubleshooting hints, not a measured improvement guarantee. Live stream, decoder and device testing is still needed to compare actual latency and picture quality.

[Documentation](index.md) · [Video and latency](video-and-latency.md) · [Troubleshooting](TROUBLESHOOTING.md)
