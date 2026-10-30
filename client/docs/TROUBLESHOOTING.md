# Troubleshooting

[Documentation](index.md)

Find the symptom, make one change and try again. For performance problems, first open **stream menu → Display → Overlay and diagnostics**, choose **Debug** and use **Copy stats**, so you can compare the numbers before and after the change. The first three tables describe the Android app; [Nightjar for Windows and Linux](#nightjar-for-windows-and-linux) has its own table.

## Add and pair a host

Start on the same local network, with the PC awake and its host running, and follow the [PIN flow](pairing.md#pin-flow).

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| The PC is not discovered | The host is stopped, a firewall blocks discovery, or discovery cannot cross the network boundary. | Start the host and add the PC with **Add PC** and its local address. Check guest Wi-Fi isolation, VPN routes and the PC's firewall. |
| A manual address fails | Wrong address or port, no route, or the host is not running. | Enter the PC's address and the streaming service's HTTP port, not the web UI's URL or `127.0.0.1`. Write IPv6 addresses with a port as `[address]:port`. |
| The PIN is rejected or pairing stalls | An old PIN, an expired one-time PIN, a wrong passphrase, or another pending request. | Cancel the attempt, clear the pending request on the host and use a new PIN. For a one-time PIN, enter both the PIN and the passphrase before it expires. |
| Pairing is gone after a restore | Backups and device transfers exclude the client identity. | Pair the restored installation again and revoke the old device on the host. |
| Paired, but no games appear | Listing apps is not permitted, the device is disabled, or the host's library is empty. | On hosts with per-device permissions, enable the device and **List applications** in the host's web UI. Check the host's app list. |
| Video works, controls do not | An input permission is missing, or the device has a view-only role. | Grant the controller, mouse, keyboard, touch or pen permission you need. Start a normal game rather than Remote Monitor. |
| A Fire TV remote cannot control the PC | The remote's D-pad and Select send keyboard input during a stream. | Grant keyboard input to this device on the host. Back and Menu open Nightjar's stream menu; use a controller for games that accept only controllers. See [TV remotes](controls.md#tv-and-fire-tv-remotes). |
| A handheld's Back button works in the app but not during a stream | The handheld sends Back from a controller input device. | Turn on **Settings → Controls → Physical Back opens stream menu**, then reconnect. See [handheld Back](controls.md#physical-back-on-a-handheld). |
| Clipboard or commands are missing | The host does not support them, or the device lacks the permission. | Use a host that supports them, such as Apollo, Vibepollo or Rubylight, and grant **Send clipboard**, **Read clipboard** or **Server commands**. Commands must be configured on the host. |

Nightjar uses the ports the host advertises, including custom port offsets; check the host's configuration rather than a generic port list. Local streaming works regardless of the result of the Internet connectivity check.

## Video, sound and latency

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Black or flickering picture, or misplaced video | A decoder or display-surface problem. | Try H.264 at 720p and 60 fps with HDR and upscaling off. For SDR streams, try **Settings → Advanced → Compatibility video view**; it is unavailable with HDR and PyroWave. |
| The decoder crashes | An unstable combination of codec, resolution and driver. | Try another codec and a lower resolution or frame rate. After repeated decoder crashes, Nightjar resets the stream settings to safe values; check the PC's own settings as well. |
| Network loss or stutter | The bitrate exceeds what the link sustains, or traffic arrives in bursts. | Lower the bitrate, connect the PC by Ethernet, or move closer to the access point. Try **Device controls bitrate** (**Settings → Video**) with a host that supports it: Rubylight, or a Sunshine-based PC with its own automatic bitrate. |
| Stutter every few minutes | Background Wi-Fi scans on the device or the PC. | Open **Why did my stream stutter?** after a few minutes of play. If it reports repeating stalls, turn off Wi-Fi and Bluetooth scanning in Android's location settings and use 5 or 6 GHz Wi-Fi. See [stream health](stream-health.md#stalls-every-few-minutes). |
| An error appears after exiting a game | The host closed the connection before its end-of-stream message arrived. | Nightjar returns to the library once the host confirms that the session ended. If the error persists, attach a new problem report and the host's exact version. |
| High **Queue wait** | The decoder's input buffers or submission thread are backed up. | Lower the resolution or frame rate, and compare with **Settings → Advanced → Phone performance hints** off. |
| High **Decode time** | Slow decoding or driver scheduling. | Compare H.264, HEVC and AV1, then lower the resolution or frame rate, or turn off HDR and 4:4:4. |
| Decode time is fine, but the rendered frame rate is low | Presentation, pacing or the display's refresh policy. | Compare **Lowest latency** pacing and adaptive refresh, and pick a frame rate the screen supports. Check power-saving modes. |
| High **Host processing** | Capture, encoding or game load on the PC. | Reduce the PC's load and check the host's logs. Settings on the device cannot reduce host processing time. |
| HDR looks washed out or is missing | A mismatch in HDR support, metadata or range. | Check that HDR is on for the PC's display and the device's screen. Set the full-range override back to **Off** and check the negotiated codec and HDR mode in Debug. |
| The upscaler reports direct output | Upscaling is off, HDR or 10-bit is active, the output is not larger than the stream, or the GPU path is unavailable. | Read the reason in Debug. Lower **Stream resolution**, for example to 720p on a 1080p screen, then choose FSR or SGSR and SDR. PyroWave supports SGSR for both. Reconnect after a decoder overload or a resize fallback. |
| PyroWave is unavailable | The device lacks 64-bit Android or the required Vulkan features, the host does not offer PyroWave, the host's codec version differs, or the surface format is unsupported. | Read the codec readiness message. PyroWave needs a host that offers it, such as Rubylight; otherwise use Automatic, HEVC or AV1. Test the link's throughput before raising PyroWave's bitrate. |
| A custom GPU driver cannot be selected | The device does not meet the requirements. | Custom drivers need Android 10 or newer, an arm64 device and an Adreno GPU. They affect PyroWave's Vulkan path, not the hardware video decoder. |
| A driver ZIP is rejected | Invalid metadata, paths, ABI or size. | Use an Adreno package with `meta.json` at the top level and arm64 libraries; see [package requirements](video-and-latency.md#snapdragon-gpu-drivers). A ZIP whose contents sit in a folder is rejected. |
| A custom driver falls back, or the app exits | Loading, Vulkan checks or initialization failed, or the app closed while the driver was active. | Choose **Settings → Advanced → Snapdragon GPU driver → System GPU driver** and start a new stream. After an interrupted session with a custom driver, the next launch resets the choice. Debug shows the driver in use. |
| **Statistics** shows **—** | The renderer or the host has not reported that value. | Wait for samples and check Debug. Some devices and hosts do not provide every value; a missing value is not zero. |
| The PC's CPU and GPU readings are missing | The host does not send them, they are off, or the stream is not eligible. | Use a host that sends hardware readings (Rubylight), turn on **Send hardware metrics to client** in its settings, reconnect, and show the overlay. The device needs its own video stream and view permission. GPU readings need AMD ADLX on the PC; CPU usage works without it. |
| The PC's readings disappear | The overlay or app went to the background, a request failed, or the sample expired. | Return to the stream and wait for a new sample. Failed requests are retried; reconnect after changing host settings or permissions. Expired readings are removed rather than shown stale. |
| Heat, battery drain or declining performance | Sustained decoding, GPU and display load. | Try **Battery saver**, a lower frame rate or resolution, or upscaling off. Compare over a sustained run, not just the first minute. |
| No sound, or the wrong channels | Output routing, a channel mismatch or the host's audio setup. | Try **Stereo**, check the device's volume and output and the host's audio device, then reconnect. |
| Audio delay or glitches | Audio effects, the output route or buffer recovery. | Turn off **Allow system audio effects**, compare another output and **Stereo**, and note whether glitches follow a change of output. |

[The performance overlay](video-and-latency.md#performance-overlay) defines each value. Network round trip plus decode time is not an end-to-end latency measurement.

## Controls, profiles and installation

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| A USB DualSense has no adaptive triggers | The USB driver is off, USB access was denied, or the host sends no trigger effects. | Turn on **Settings → Controls → USB DualSense driver**, reconnect the cable and allow USB access when asked during a stream. Over Bluetooth the controller uses Android's input path. |
| Mouse look behaves like a desktop cursor | **Desktop mouse mode** is on, or input is not captured. | Turn **Desktop mouse mode** off and press **Ctrl+Alt+Shift+Z** to capture input again. |
| A stick drifts | The dead zone is too small for that controller or game. | Increase **Analog stick deadzone** slightly and compare. |
| Touch behaves like a mouse | Trackpad or direct mouse mode is selected, or the host has no native touch. | Select multi-touch and grant **Touch input** on the host. Without native touch on the host, Nightjar uses direct mouse. |
| A preset or global change seems ignored | This PC has its own value for that setting. | Long-press the PC, open **PC settings…**, set the value to **Same as all PCs** and save. |
| A reset kept a controller mapping or layout | A global reset preserves them by design. | Reset the controller under **Controller buttons**, or use **Reset controls layout**. |
| An APK update is rejected | A different signing key or an incompatible build. | Install an update signed with the installed app's key. Uninstalling allows a different key but removes local data and the pairing identity. |
| The PC does not wake | No MAC address saved, or a network adapter, firmware, Wi-Fi or routing limitation. | Pair once while the PC is awake, enable Wake-on-LAN, and test from sleep over Ethernet on the same subnet. |
| A launcher cannot open a game file | A missing file permission, an invalid entry or an unpaired PC. | Export again after opening the paired PC's library; follow the [launcher setup](FRONTENDS.md). |

## Nightjar for Windows and Linux

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| PCs are not found | Windows Firewall blocks Nightjar, or discovery cannot cross the network. | Allow Nightjar on private networks when Windows asks. Add the PC by address with **Add PC**. |
| The stream reports a fallback to software decoding | No hardware decoder supports this codec, resolution or format. | Try HEVC or H.264, lower the resolution, or turn off **10-bit color (SDR)** and **Full chroma**. On Linux, install the VA-API driver for your GPU. |
| **Frame pacing** and **V-Sync** are unavailable | **Variable refresh rate** is on, so the display paces frames. | Turn **Variable refresh rate** off to choose **Frame pacing** and **V-Sync** yourself. |
| A setting has no effect on Linux | Codec, upscaling, **Prefer YUV 4:4:4**, **10-bit color (SDR)** and **Full chroma** take effect after a restart on Linux. | Restart Nightjar. |
| Loss in bursts on Linux, with a receive buffer note in **Why did my stream stutter?** | The system limits the video socket's receive buffer. | Run the `sysctl` command given in the note, then start a new stream. |
| No echo cancellation on Linux | The PipeWire or PulseAudio echo-cancel module is not loaded. | Load the module for your sound server, or use a headset. |
| A USB device shows another driver on Windows | Nightjar forwards USB devices through WinUSB only. | Install WinUSB for the device, for example with Zadig, to forward it. |
| Anything else | | Open **Settings → App → Logs → Open logs folder** and attach `nightjar.log`, and `nightjar.prev.log` if the problem occurred before a restart, to a [bug report](https://github.com/RamazanKara/nightjar/issues/new/choose). |

## Connection test

Long-press a PC and choose **PC settings… → Connection test**. The test makes ten timed server-info requests. Ping is their average round trip; jitter is the average change between consecutive successful requests; loss is the share of failed requests, not video packet loss.

With a paired host that supports it (Rubylight, or Vibepollo with PyroWave available), an optional 32 MiB HTTPS download measures throughput and shows the link speed the host's network adapter reports. Other hosts get the delay test only, and the test's first screen says which applies. The test uses network data and briefly competes with other traffic; cancel or leave it to stop. It never changes stream settings.

On Windows and Linux, **Play away from home → Remote access → Test this PC** in the PC's details verifies the paired PC with five small HTTPS requests.

## Export a problem report

1. Reproduce the problem and note the app version, the device and its Android version, the host's version and the time.
2. Open **Settings → App → Report a problem**, select **Share** and choose an app in Android's share sheet.
3. Where relevant, add **stream menu → Display → Overlay and diagnostics → Copy stats** and **Settings → Advanced → Export latency CSV**. Copy stats includes every Debug value.
4. For pairing and host problems, long-press the PC, open **PC settings… → View details → Copy debug info**, and add the host's log.
5. Describe what you did, what you expected and what happened in a [Nightjar issue](https://github.com/RamazanKara/nightjar/issues).

The report is a text file, `nightjar-problem.txt`. It contains up to 200 redacted events from the running app, the last decoder configuration and the last saved crash record. Addresses, names given to PCs and devices, PINs, credentials and free-text diagnostic details are removed. Nothing is uploaded: the report leaves the device only through the app you choose. Review host details and logs you copy separately before sharing them.
