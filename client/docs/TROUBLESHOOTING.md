# Troubleshooting

[Documentation](index.md)

Use the symptom below, then make one change and retry. For performance issues, open **stream menu → Display → Overlay and diagnostics**, choose **Debug** and **Copy stats** before changing settings.

## Add and pair a host

Follow the [PIN flow](pairing.md#pin-flow). Start on the same local network with the PC awake and its streaming host running.

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| PC is not discovered | Discovery cannot cross the network boundary, the host is stopped, or a firewall blocks it. | Start the host; use **Add PC** with its local address. Check guest Wi-Fi isolation, VPN routes and the host firewall. |
| Manual address fails | Wrong address/port, no route, or service unavailable. | Enter the PC's address and streaming HTTP port, not the host's web UI URL or Android's loopback address. For custom IPv6 ports use `[address]:port`. |
| PIN rejected or pairing stalls | Old PIN, expired one-time PIN, wrong passphrase or another pending request. | Cancel the attempt, clear the pending host request and use a fresh PIN. For a one-time PIN, enter both the host PIN and passphrase before expiry. |
| Pairing disappears after restore | Client identity is excluded from backup/device transfer. | Pair the restored installation again and revoke the obsolete device on the host. |
| Paired but no games appear | Application listing is denied, device disabled or host library empty. | If your host has per-device permissions, enable the device and List applications in its settings or web UI; check the host library. |
| Video works but controls do not | Missing input permission or a view-only role. | Grant the required controller/mouse/keyboard/touch/pen permission. Open a normal game instead of Remote Monitor. |
| Fire TV remote cannot control the PC | Remote D-pad/Select use keyboard input while streaming. | Grant keyboard input to this device on the host. Back/Menu open Nightjar's stream controls; use a gamepad for controller-only games. See [TV remotes](controls.md#tv-and-fire-tv-remotes). |
| Handheld Back works in the app but not during a stream | The physical Back key arrives through a gamepad input device. | Enable **Settings → Controls → Physical Back opens stream menu**, then reconnect. See [handheld Back](controls.md#physical-back-on-a-handheld). |
| Clipboard or commands missing | Independent permissions or host capabilities are absent. | Use a host that supports them (e.g. Apollo or Rubylight) and grant Send clipboard, Read clipboard or Server commands as appropriate. Commands must be configured on the host. |

The client uses the host's advertised ports and custom-port offsets. Check the actual host configuration instead of opening a copied port list. Local streaming works independently of the public Internet connectivity check.

## Video, sound and latency

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Black/flickering image or misplaced video | Decoder or surface-path issue. | Try H.264 at 720p/60 with HDR/upscaling off. For conventional SDR, try **Settings → Advanced → Compatibility video view**. It is disabled for HDR/PyroWave. |
| Decoder crash | Codec, resolution or driver combination is unstable. | Try another codec and lower resolution/FPS. After repeated decoder crashes Nightjar resets stream settings to safe values; check the PC override too. |
| Network loss or stutter | Bitrate exceeds a stable link budget, or traffic is bursty. | Lower bitrate; wire the PC; move closer to the access point. Try **Device controls bitrate** (Settings → Video) on a host that supports it (currently Rubylight). |
| Stutter every few minutes | Background Wi-Fi scans on the phone or PC. | Open **Why did my stream stutter?** after a few minutes of play; if it reports repeating stalls, turn off Wi-Fi and Bluetooth scanning under Location settings and use 5 or 6 GHz Wi-Fi. See [Stream health](stream-health.md#stalls-every-few-minutes). |
| Disconnect error after exiting a game | The host closed the transport before its normal end-of-stream message arrived. | Update to Android 0.4.11 or newer. Nightjar returns to the library when the paired host confirms the session has ended, with **Recover interrupted streams** on or off. If it still fails, attach a fresh **Settings → Report a problem** log and the exact host version. |
| High Queue wait | Decoder input buffers or the submission thread are backed up. | Lower resolution/FPS and compare with **Settings → Advanced → Phone performance hints** off. |
| High Decode time | Decoder/driver scheduling or decoding is slow. | Compare H.264, HEVC and AV1, then reduce resolution/FPS or disable HDR/4:4:4. |
| Healthy decode but low rendered FPS | Presentation, pacing, callback availability or display policy. | Compare Lowest latency/VRR and a rate the screen supports. Check power-saving modes. |
| High Host processing | PC capture/encoder/game workload. | Reduce the host workload and compare host logs; Android decoder settings cannot remove host processing time. |
| HDR is washed out or absent | HDR capability, metadata or range mismatch. | Verify HDR on the host display and Android screen; return full-range override to Off and inspect the negotiated codec/HDR mode. |
| Upscaler reports direct output | Upscaling is off, HDR/10-bit is active, there is no size increase, or the GPU path is unavailable. | Read the reason in Debug. Lower **Stream resolution**, for example 720p on a 1080p output, then choose FSR/SGSR and SDR. PyroWave supports SGSR for both selections. Reconnect after a MediaCodec overload/resize fallback. |
| PyroWave unavailable | Unsupported Android ABI/Vulkan features, a host without PyroWave support, host codec version or surface format. | Read the codec readiness message. PyroWave needs a host that supports it (currently Rubylight); otherwise, and on unsupported devices, use Automatic/HEVC/AV1. Test link throughput before raising PyroWave bitrate. |
| Custom GPU driver cannot be selected | Device/app does not meet the custom-driver requirements. | Custom drivers require Android 10+, arm64 and an Adreno GPU. Other devices use the system driver. This option affects PyroWave's Vulkan path, not MediaCodec. |
| Driver ZIP is rejected | Invalid metadata, paths, ABI or package size. | Use a compatible Adreno package with root-level `meta.json` and arm64 libraries; see [package requirements](video-and-latency.md#snapdragon-gpu-drivers). A ZIP containing a parent folder is rejected. |
| Custom driver falls back or the app exits | Driver loading, Vulkan checks or initialization failed, or the driver was active when the process exited. | Select **Settings → Advanced → Snapdragon GPU driver → System GPU driver**, then start a new stream. After an interrupted custom-driver session, the next launch resets that selection automatically. Debug shows the driver actually used. |
| Statistics shows **—** | The renderer/host has not provided that metric. | Wait for samples and inspect Debug. Display callbacks, host timings and individual hardware readings may be unavailable; a missing value is not zero. |
| Host CPU/AMD GPU readings are missing | Host opt-in/capability, permissions, stream eligibility or driver support is absent. | Use a host that supports it (currently Rubylight) and enable **Send hardware metrics to client** in its settings (on Rubylight: **Settings → General → Hardware metrics**), reconnect a normal/Remote Monitor stream and show the overlay. The paired device needs its own active video stream and viewer permission. GPU readings require AMD ADLX; CPU usage can work independently. Check the host's metric selection. |
| Host hardware readings disappear | The overlay/app became inactive, a request failed or the sample became stale. | Return to the foreground overlay and wait for a fresh sample. Temporary failures retry with backoff; reconnect after changing host settings or permissions. Disappearing metrics are cleared instead of displaying old values. |
| Heat, battery drain or degraded performance over time | Sustained high decode/GPU/display workload. | Try Battery saver, lower FPS/resolution or turn upscaling off. Compare a sustained run, not only the first minute. |
| No sound or wrong channels | Output routing, surround mismatch or host audio configuration. | Try Stereo, confirm Android volume/output and the host audio device, then reconnect. |
| Audio delay or glitches | Effects, output route or buffer recovery. | Disable Allow system audio effects, compare another output route and Stereo, and record whether glitches follow a route change. |

See [video and latency](video-and-latency.md#performance-overlay) for metric definitions. Network RTT plus decode time is not an end-to-end latency measurement.

## Controls, profiles and installation

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| USB DualSense has no adaptive triggers | Direct USB driver is off, access denied, or host feedback unavailable. | Enable **Settings → Controls → USB DualSense driver**, reconnect the cable and allow USB access during a stream. Check host support. Bluetooth uses Android's input path. |
| Mouse look behaves like a desktop cursor | Desktop mouse mode or capture state. | Turn Desktop mouse mode off and use Ctrl+Alt+Shift+Z to recapture input. |
| Controller stick drifts | Deadzone too small for that pad/game. | Increase Analog stick deadzone slightly and compare. |
| Touch behaves like a mouse | Trackpad/Direct mouse selected, or native touch unavailable. | Select Multi-touch and grant Touch input on the host. A host without native touch falls back to Direct mouse. |
| Preset or global change seems ignored | This PC has its own value for that setting. | Long-press the PC, open **PC settings…**, set the value to **Same as all PCs**, and save. |
| Reset did not remove a controller mapping or layout | Global reset intentionally preserves them. | Reset the controller in Controller buttons or use Reset controls layout. |
| APK update is rejected | Different signing key or incompatible build. | Use an update signed by the installed app's key. Uninstalling permits a different key but removes local app data and pairing identity. |
| PC will not wake | Missing MAC, NIC/firmware policy, Wi-Fi or routing limitation. | Pair once while awake; enable Wake-on-LAN; test Ethernet and sleep on the same subnet. |
| Frontend cannot open a game file | Missing URI permission, invalid entry or unpaired PC. | Re-export after opening the paired PC's library; follow [frontend setup](FRONTENDS.md). |

## Connection test

Long-press a PC and choose **PC settings… → Connection test**. The test makes ten timed server-info requests. Ping is their average round trip; jitter is the average change between consecutive successful requests. Loss is the share of requests that failed, not video packet loss. When the speed is not measured, the result says why.

On a paired host that supports it (Rubylight, or Vibepollo with PyroWave available), the optional 32 MiB HTTPS download reports throughput and the host adapter's reported link speed. Other hosts get the delay test only; the test's first screen says which. It uses network data and may temporarily compete with other traffic. Cancel or leave the test to stop it. None of these measurements changes stream settings.

## Export a problem report

1. Reproduce the problem and note the app version, device/Android version, host version and time.
2. Open **Settings → App → Report a problem**, select **Share** and choose a receiving app in Android's share sheet.
3. Attach **stream menu → Display → Overlay and diagnostics → Copy stats** and **Settings → Advanced → Export latency CSV** when relevant. Copy stats includes all Debug details.
4. For pairing/host issues, long-press the PC, use **PC settings… → View details → Copy debug info**, and add the relevant host log.
5. Describe what you did, what you expected and what occurred in a [Nightjar issue](https://github.com/RamazanKara/nightjar/issues).

The report is a text file named `nightjar-problem.txt`. It contains up to 200 redacted events from the current app process, the last decoder configuration and the last saved crash record. Addresses, user-assigned host/device names, PINs, credentials and free-text diagnostic details are omitted. Nothing is uploaded automatically; sharing begins only when you choose a receiving app. Review separately copied host details/logs before sharing them.
