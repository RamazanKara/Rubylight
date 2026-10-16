# Configuration

[Docs](README.md) · [Getting started](getting-started.md) · [Troubleshooting](troubleshooting.md)

Configure the current **Windows Rust host** in the Rubylight console, normally at **https://localhost:47990**. Open **Settings** and search by a setting's name or configuration key. Resolution, stream frame rate, bitrate and codec are normally chosen in the client.

Start with the defaults below. Settings imported from an older installation keep their saved values; upgrading does not reset them to these defaults.

## Find and review settings

Search includes settings hidden by another choice, advanced options and configuration keys. Each hidden result explains when it applies. For example, search `amd_quality` to change AMD quality while keeping **Encoder → Automatic**; the value applies whenever Automatic selects AMF. Changing the encoder is not required just to find its options.

Use **Review changes** to see pending edits across all categories, including settings hidden after another edit. If saving is blocked by an invalid value, **Review errors** opens the fields to correct. Switching categories or clearing search keeps your edits. **Reset to default** removes the saved override when you save; **Undo** restores the value from before the edit.

## Defaults worth knowing

| Setting / key | Default | What it means |
| --- | --- | --- |
| Capture method — `capture` | `auto` | On a virtual display Rubylight created, takes each frame straight from Rubylight's display driver. Elsewhere prefers Windows Graphics Capture (WGC), with Desktop Duplication (DDX) as fallback. |
| Compute conversion — `gpu_compute_conversion` | `true` | Uses a separate compute queue for supported AMD capture and colour conversion paths. |
| Encode from GPU memory — `wgc_direct_encoder_input` | `true` | Keeps captured frames on the GPU instead of copying them through system memory. |
| Encoder — `encoder` | `auto` | Selects an available encoder for the capture GPU. |
| AMD usage — `amd_usage` | `ultralowlatency` | AMD's lowest-latency preset. The driver also turns on its internal low-latency mode, and the lowest latency for AV1. |
| AMD quality — `amd_quality` | `speed` | Favours low encoding latency. |
| Frame pacing — `frame_pacing` | `arrival` | Encodes arriving frames, limited to the requested stream rate. |
| Use the launch frame rate — `limit_framerate` | `false` | Off, the stream runs at the rate the stream request asks for, as in Vibepollo; a virtual display's refresh and the frame limiter follow it. On, the launch request's rate is encoded, as before. Artemis's Warp frame pacing launches at 60 and asks for 120 or 240. The client's bitrate is the budget either way. |
| Video packet pacing — `pacing_max_bitrate_kbps` | `0` | Route-based: fast wired pacing; confirmed host Wi-Fi uses twice the encoder bitrate with FEC headroom. Positive values set a fixed limit. PyroWave keeps its separate policy. |
| Adaptive packet pacing — `host_adaptive_pacing` | `false` | Experimental opt-in: slower startup, tentative rate increases and loss-based rollback. Does not change picture bitrate or FPS. |
| Slow down after lost keyframes — `keyframe_loss_pacing` | `true` | Once a keyframe does not reach the client, paces video like Wi-Fi (twice the encoder bitrate) for the rest of the stream. Off with a manual pacing limit or either adaptive option. |
| Host bitrate adaptation — `host_adaptive_bitrate` | `false` | Experimental loss adaptation for H.264, HEVC and AV1. Requires client feedback and yields to client bitrate control. [Details](#host-bitrate-adaptation). |
| Audio to stream — `audio_capture_mode` | `system` | Captures the selected playback device. Optional [Game only](#game-only-audio) captures a verified game's process tree. |
| HEVC / AV1 support — `hevc_mode`, `av1_mode` | `0` | Automatic capability detection; only working profiles are offered. |
| PyroWave — `pyrowave` | `true` | Offers PyroWave to clients that support it. |
| Virtual display — `virtual_display_mode` | Windows 11: `per_client`; Windows 10: `disabled` | A display for each device on Windows 11; a physical monitor on Windows 10. Requires the virtual display driver when enabled. |
| Virtual display layout — `virtual_display_layout` | `exclusive` | Turns other displays off while the virtual display is used. A Remote Monitor follows it while it streams: exclusive turns the other displays off and makes it primary, the primary layouts make it primary, and it keeps the place the remote monitor layout gives it. Choose Extended to use a remote monitor beside your own. |
| Physical display preparation — `dd_configuration_option` | `verify_only` | Requires the selected monitor to be active; resolution, refresh and HDR policies are separate. |
| Display HDR — `dd_hdr_option` | `auto` | Matches the streamed display's HDR state to the stream. |
| Limit every stream — `frame_limiter_enable` | `false` | Manual limiting is off; virtual-display automatic limiting can still apply. |
| Limiter — `frame_limiter_provider` | `auto` | Uses RTSS when installed, otherwise the NVIDIA driver where available. |
| Virtual display refresh — `frame_limiter_auto_virtual_framegen` | `legacy` | Runs the virtual display at twice the stream rate and enables automatic frame limiting. |
| Update checks — `update_check_interval` | `86400` seconds | Checks once a day. |
| Automatic installation — `auto_update` | `false` | Announces available updates; installation is opt-in. |
| Include pre-releases — `notify_pre_releases` | `false` | Release candidates are offered only when enabled. |

These are defaults for **unset** keys, not a list of values you must copy into a configuration file.

## Capture and video

**Graphics card** (`adapter_name`) lists all hardware GPUs reported by Windows DXGI, including GPUs with no attached monitor on hybrid systems. **Automatic** uses the GPU that owns the captured display. A GPU appearing in the list does not confirm encoder or codec support; that still depends on its hardware and driver. With a GPU selected, virtual displays are created to render on that GPU, as Apollo and Vibepollo do. WGC can capture a display that another GPU renders; Desktop Duplication cannot, so when it is the capture path in use and the display renders elsewhere (a physical monitor on the integrated GPU, or a driver that ignored the request), the stream captures and encodes on the display's GPU and the stream card says so.

**Keep Capture method on Automatic and compute conversion enabled** for normal use. On supported AMD hardware, the compute path lets capture and colour conversion run alongside graphics work. Unsupported textures can fall back to the graphics path. The internal `wgc_compute_copy` switch also defaults to `true`; it is primarily useful for controlled troubleshooting.

**Frames from Rubylight's display driver.** On a virtual display Rubylight created, Automatic captures from the driver itself: it copies each frame Windows composes into textures it shares with the host, so neither WGC nor Desktop Duplication runs, and on AMD the compute conversion reads those textures in place (`idd_push_zero_copy`, default `true`). To select it explicitly, choose **Settings → Video → Capture method → Direct driver push** (`capture = idd-push`). It applies to Rubylight's own virtual displays and falls back to WGC when unavailable, including on a physical monitor. Choosing WGC or Desktop Duplication explicitly requests those methods. The stream captures with WGC instead, and the stream card says why, when the driver is not installed, when another stream with different capture settings already captures that display, or when the display's frame push failed twice before delivering a picture. A display that failed is tried again once it is recreated or renders on another GPU. Save and end all streams using the display before starting a new session to compare capture methods; simultaneous streams may share an already-open capture.

The installed service runs WGC through a helper in the signed-in user's session. WGC can fall back to DDX when unavailable. Selecting a particular capture method is useful when investigating a problem; check **Logs** to see which path actually opened.

H.264, HEVC and AV1 depend on the encoder and client. For `hevc_mode` and `av1_mode`, the choices are `0` Automatic, `1` Off, `2` SDR only and `3` SDR and HDR. Restart after changing advertised codec support. There is no 4:4:4 switch: the host offers 4:4:4 for a codec only when the startup check encodes it on a hardware encoder. AMD's encoder makes 4:2:0 only, so an AMD host offers 4:4:4 through PyroWave alone.

For **native HDR**, enable HDR in the client and use an HDR-capable display and encoder path. Leave **Display HDR** on Automatic so Rubylight can set the source display appropriately. **10-bit SDR instead of HDR** (`prefer_sdr_10bit`, default `false`) deliberately keeps the stream in SDR; leave it off when you want HDR.

**PyroWave needs a compatible client** and much more bandwidth than conventional codecs. Desktop and game test scenes on AMD give two warning levels. The floor warns about severe detail loss; the recommendation targets clean pictures in those tests. Passing the floor alone does not mean a clean picture.

| Stream | Severe-loss floor | Recommended |
| --- | ---: | ---: |
| 720p60 | 139 Mbps | 277 Mbps |
| 1080p60 | 187 Mbps | 399 Mbps |
| 4K60 | 747 Mbps | 1593 Mbps |

These values are rounded up to whole Mbps. Below 1080p the floor/recommendation are 2.5/5 bits per pixel per frame; at 1080p and above they are 1.5/3.2. The smaller desktop's one-pixel text needed more bits per pixel. Rates scale with pixel count and frame rate within each band: halve them at 30 fps, double them at 120 fps. Other sizes are estimates; actual quality depends on text size, textures and the picture. The console stream card uses red below the floor and amber below the recommendation, and the log distinguishes both. The [October 12, 2026 Rubylight 2.2.9 validation](../rust/PERFORMANCE.md#pyrowave-decoded-pictures-and-quality-budgets) rechecked these budgets on synthetic desktop and game pictures; it measures decoded quality, not sustained network throughput. [Original October 7 method and criteria](../rust/PERFORMANCE.md#october-7-pyrowave-bitrate-from-representative-pictures).

The PyroWave bitrate you set in the client is the stream's total on the wire, as in Vibeshine. The host keeps audio with its parity and 500 kbps for control. With record framing (Nonary's Moonlight, Nightjar, Aurora), each picture then gets what fits after packet overhead and one block of critical FEC, so a rate the client calibrated is never exceeded. The picture therefore gets about 15–20% less than the number you set; set the bitrate to what your network carries, not to the picture rate you want. The 4K60 recommendation needs more than gigabit Ethernet. Client and host limits still apply: stream setup allows up to 2 Gbps, while the client's runtime `/bitrate` endpoint caps changes at 500 Mbps. If the client or network cannot carry the recommended rate, use HEVC or AV1. Enabling PyroWave does not force ordinary Moonlight clients to use it. NVIDIA users should use [Vibepollo](https://github.com/Nonary/Vibepollo).

**A Rubylight client can change the stream's resolution and frame rate without reconnecting**, for example when a foldable phone opens its inner screen. The encoder restarts at the new size with a keyframe and pacing follows the new rate; the PC's display keeps its mode and the picture is scaled to fit. `stream_reconfigure` (default `true`) turns this off for the host, a device or an app; clients that never ask are unaffected. [Control messages](control-messages.md) describes the request.

**VRR is client-negotiated.** A VRR request can use a 1000 Hz virtual display when automatic virtual refresh is enabled. That is the host's virtual display rate, not a claim that your TV or monitor refreshes at 1000 Hz. Client and display support still matter.

**Packet pacing limits video bursts.** The default remains the existing route-based policy: with `pacing_max_bitrate_kbps = 0`, H.264, HEVC and AV1 use up to 800 Mbps on wired or unknown routes. Confirmed host Wi-Fi uses twice the encoder bitrate, with at least 25% headroom above packetized demand. A known physical Ethernet link limits pacing to 80% of its reported speed. Hyper-V vSwitches use their physical adapter when Windows exposes an unambiguous binding. The host's link speed does not establish the capacity of the client or the rest of the path.

**A client that loses keyframes is slowed down** (`keyframe_loss_pacing = true`). Behind a port slower than the host, such as a TV box's 100 Mbps Ethernet after a switch with a shallow queue, a keyframe sent at the wired rate overflows the queue; the client asks for another, which is lost the same way, and the picture freezes. When the client reports a keyframe it could not repair, or asks again for the keyframe it was just sent, the stream is paced like a confirmed Wi-Fi route for the rest of the session: twice the encoder bitrate, at least 1.25 times wire demand. A client that never loses a keyframe keeps the fast policy. Through a 100 Mbps loopback bottleneck with a 16 or 64 KiB queue, 1440p60 AV1 and HEVC streams at 40 and 60 Mbps went from no frame received whole in 20 seconds to all but the first few; without a bottleneck nothing changed. [Measurements](../rust/PERFORMANCE.md#october-14-keyframe-loss-on-a-slow-port).

**Adaptive packet pacing is experimental and off by default** (`host_adaptive_pacing = false`). It is an option for connections with burst-related loss when manual pacing is inconvenient. Enabling it starts each conventional stream at twice the encoder bitrate or 1.25 times wire demand, whichever is higher, within the route ceiling. A suitable fixed limit is simpler when the constrained path is known. Neither policy establishes the cause of an Onn client's reported problem; that device has not been validated.

During startup, busy video, recent standard client pings and stable nonzero RTT allow tentative increases of up to 12.5%, separated by at least half a second and a feedback-delay guard. A new loss report during a probe restores the preceding rate and switches to slower recovery. There is no timer-only jump to the maximum. Each reconnect starts conservatively again: a client can move between Ethernet and Wi-Fi without changing its identity or the host's route. Startup pacing adds transmission time, especially to a large first keyframe; with a 40 Mbps encoder and continuously healthy traffic, reaching an 800 Mbps ceiling takes about ten seconds.

Outside tentative probes, the optional controller reduces bursts after repeated, validated client FEC reports show substantial loss. It changes packet spacing without lowering encoder bitrate or FPS. A half-second sample needs two distinct failed frames, or multiple recovered frames with at least 2% of sent packets missing. Old and duplicate reports cannot repeatedly penalize a replacement rate. The rate stays at least 25% above current packetized video demand, including FEC, minimum parity, encryption and network headers, unless the route ceiling is already lower. This floor prevents pacing itself from creating a persistent backlog; it cannot make an overloaded path carry the stream.

Recovery needs recent standard client pings, active video traffic, stable RTT and an initial eight-second clean hold. Small rate increases are tentative; a failed probe restores the previous rate and extends the next recovery hold, up to a minute. Paused video, absent standard pings or missing RTT does not authorize a faster rate. Stock Moonlight does not acknowledge every successfully received video frame: control pings establish activity, not video-path capacity. Loss during probing remains possible, and clients without loss reports cannot trigger loss-based rollback. The session API exposes `packet_pacing`, including startup state, and rate changes appear in the host log. The separate **Host adaptive bitrate** option keeps its existing opt-in quality/bandwidth controller and disables this additional burst controller.

Conventional video batches contain at most about one millisecond of traffic at the selected rate, or one datagram if it takes longer. `video_max_batch_size_kb` remains an additional payload limit (16, 32 or 64 KiB; default 64). With `video_send_thread` enabled (the default), adaptive packet pacing reserves the dedicated video sender from the start so pacing waits do not block capture. Large keyframes can still take multiple frame periods. Startup and steady-state timing are reported separately in the measurement record.

Set `pacing_max_bitrate_kbps` to a positive value in **kbps** to override automatic burst control, including for PyroWave. For example, `120000` paces at 120 Mbps. The existing floor of 110% of the stream bitrate still applies, and a known Ethernet link can lower the limit to 80% of link speed. Explicit limits are not raised for FEC; the host warns if the wire budget exceeds the resulting rate. `800000` keeps a fixed 800 Mbps ceiling when the physical link permits it. Reconnect after saving. PyroWave's automatic policy retains 95% of a known Ethernet link, or 1.25 times its per-frame wire demand (at least 110% of the stream bitrate) on other routes. [Measurements and pacing math](../rust/PERFORMANCE.md#october-14-conventional-video-pacing).

**Leave AMF's low-latency mode and AV1 latency mode on Driver default.** In the [October 12, 2026 Rubylight 2.2.9 property sweep](../rust/PERFORMANCE.md#retained-amf-property-sweeps-october-12), default ultra-low-latency usage already read back internal low latency for H.264/HEVC and AV1 latency mode 3 on the RX 7900 XT. Explicitly forcing the same settings produced similar timings across three runs, not exact equality or a demonstrated speedup. With Low latency or Transcoding usage, single auto/forced pairs changed the readbacks from off to on and showed mean reductions of 0.30-0.31 ms for HEVC and 1.01-1.37 ms for AV1. These are eight-second synthetic 1440p120/20 Mbps component tests, not full-stream latency measurements. The [October 7 audit](../rust/PERFORMANCE.md#october-7-amf-low-latency-mode-and-av1-latency-mode) documents earlier measurements and the RX 9000 HEVC-freeze concern; forcing these settings remains opt-in.

**Adaptive quantization (`amd_vbaq`) is off for H.264 and on for HEVC and AV1 when unset.** In the [October 12, 2026 Rubylight 2.2.9 settings comparison](../rust/PERFORMANCE_WORK.md#current-amf-quality-settings-comparison), explicitly turning H.264 AQ on reduced VMAF in all four cases: by 1.44 and 11.89 points on the two clips at 1440p120/20 Mbps, and by 0.06 and 0.41 at 50 Mbps. Turning AQ off gave mixed HEVC results, while AV1 VMAF rose in all 14 tested cases by 0.0009-0.2301 points in SDR and 0.0051-0.4822 in converted HDR. Those small AV1 gains do not establish a universal visual-quality benefit; the HDR scores also use converted SDR footage. Set AQ to On or Off to compare your own content, or Driver default to leave it to the driver. The [October 8 findings](../rust/PERFORMANCE_WORK.md#amf-settings-per-codec-quality-at-a-given-bitrate) remain a separate historical result.

The [current default-quality results](../rust/PERFORMANCE_WORK.md#current-amf-default-quality) cover 32 cases; the [settings comparison](../rust/PERFORMANCE_WORK.md#current-amf-quality-settings-comparison) adds 116 alternatives for quality preset, rate control, AQ, usage and high-motion boost. Each case encoded and scored exactly 180 frames, with no minimum VMAF pass threshold. CBR increased p99 frame size in every tested case; its score changes depended on codec and content. Ten high-motion-boost requests completed but lacked effective-setting readback, so their application is unverified. Encode-time deltas use earlier baselines rather than randomized matched trials. HDR rows convert SDR footage with 203-nit white and do not establish native-HDR perceptual quality.

**Leave split-frame encoding on Automatic.** AMF can split one HEVC or AV1 frame across a Radeon's two encoder engines (`amd_split_frame`), and the driver decides whether it does. Automatic asks for it only when the GPU has two engines and the driver has it off; On asks whenever there are two engines, and Off disables the property. In the [October 12, 2026 Rubylight 2.2.9 sweep](../rust/PERFORMANCE.md#retained-amf-property-sweeps-october-12), RX 7900 XT readbacks were 1 for Automatic/On and 0 for Off. Identical-input synthetic throughput was similar: for example, two-run means for 4K HEVC were 197.32 / 197.37 / 197.36 FPS respectively. This does not mean every frame had identical timing or prove how the driver used its engines. The sweep completed 708/720 calls; twelve pre-analysis cases failed and remain documented. It did not assess decoded quality. H.264 has no split-frame property; single-engine GPUs get no property write. [Historical October 7 measurements](../rust/PERFORMANCE.md#october-7-amf-split-frame-encoding) remain separate.

**AMF rate-control limits are optional.** In **Settings → Encoders → AMD AMF**, the advanced controls below use the client's requested bitrate and frame rate. All default to `0`, which leaves the corresponding property to the driver. Reconnect after saving. An unsupported explicit request is reported as an AMF setting error; the effective values appear in the `AMF encoder settings` log line.

| Key | Values | Meaning |
| --- | --- | --- |
| `amd_peak_bitrate_ratio` | `0`, or `1`–`2` | Peak bitrate as a multiple of the stream bitrate; the console offers 1×, 1.5× and 2×. This is not an individual-frame cap. |
| `amd_vbv_buffer_frames` | `0`, or `0.5`–`2` | Rate-control buffer in nominal frame budgets. One budget is bitrate divided by frame rate, including fractional rates. |
| `amd_max_frame_size` | `0`, or `1`–`8` | Requested maximum encoded frame size in nominal frame budgets, including recovery keyframes. Uses `MaxAUSize` for H.264, `HevcMaxAUSize` for HEVC and `Av1MaxCompressedFrameSize` for AV1. |

These are encoder bit budgets, not extra queued frames or packet-pacing settings. A smaller budget trades picture quality for smaller frames. Intra refresh remains client-negotiated and does not replace an explicit recovery-keyframe request. See the [rate-control measurements](../rust/PERFORMANCE.md#october-7-2026-amf-rate-control-and-recovery-keyframes) before changing these controls. NVIDIA users should use [Vibepollo](https://github.com/Nonary/Vibepollo).

`amd_rc` remains `vbr_latency` by default. For an affected AMD stream, `amd_max_frame_size=1` or `2` lets you compare smaller recovery frames against picture quality; `0` restores the driver default. These caps cut the largest recovery frames sharply. They are off by default because they trade picture quality for smaller frames.

## Host bitrate adaptation

Available in Rubylight 2.2.16 and later.

**Settings → Video → Adapt bitrate on the host (experimental)** enables
`host_adaptive_bitrate = true` for the next connection. It is off by default.
For H.264, HEVC and AV1, the host reduces the video budget when a Moonlight
client reports loss, or when sustained traffic shows a rising control RTT.
The budget includes packet headers, FEC (including minimum parity), and a
reserve for audio and control. Video pacing enforces the remaining video
budget; bitrate targets leave additional headroom for encoder overshoot.
Observed controller speaker, actuator and microphone PCM each reserve another
2 Mbps, including transport overhead and shared wireless airtime. Each reserve
expires after five seconds without that path's packets, including controller
removal or microphone mute. Released headroom still follows slow bitrate
recovery; it never takes ownership back from a client's controller.
The ceiling is the client's configured network bitrate, bounded by Maximum
bitrate when set. No change can exceed the initial encoder bitrate.
If the budget cannot fit negotiated audio and even minimum video packets,
adaptation remains inactive with a warning instead of slowing the pacer to an
unusable rate. Lower the audio channel count/FPS or increase the budget.

The controller starts only after a valid report for a recently sent video
frame. Clients without those reports retain their existing behavior. It waits
for eight seconds of busy video, recent control pings and stable RTT before
raising the budget slowly. A static desktop, a stopped game, missing feedback
or a quiet encoder cannot prove that more bandwidth is available. PyroWave
keeps its existing controller. A successful client `/bitrate` request takes
ownership for the rest of that connection, including removal of the host's
adaptive pacing cap. Clients starting their own controller should send their
current bitrate once to claim ownership immediately.

Session API reports include `adaptive_bitrate` with the owner, reason, observed
feedback and wire budget. This is an experimental controller validated with
simulated loss, traffic, stale feedback and ownership tests. Device/network
performance still needs live measurements; it does not promise lower latency
on every connection.

## Game-only audio

Available in Rubylight 2.2.16 and later; it uses the normal audio stream and does
not require a Rubylight-specific client.

**Settings → Audio → Audio to stream** defaults to **System audio**, which keeps
the existing playback-device capture and routing. Select **Game only**
(`audio_capture_mode = game`) to capture the launched game's process tree using
[Windows application loopback](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/).
Reconnect to apply; this setting also supports app and device overrides.

This mode requires Windows build 20348 or newer. Direct executable launches
follow their owned process job. Steam library entries use the game's existing
process tracker; Playnite uses its confirmed game path and excludes processes
present before launch. Creation times and held process handles prevent PID
reuse from selecting another application. If a launcher exits, its remaining
verified game child becomes the capture root.

Game-only audio currently captures stereo, mapped to the negotiated layout.
It leaves the host's playback devices and volume unchanged, so local game sound
can still be audible even when the client requests local muting. Route the game
to a virtual device yourself if local silence is needed. Other independent
applications and system notifications are excluded; audio from children the
game itself launches is included by Windows' process-tree capture.

Dedicated USB controller speaker forwarding is suppressed in this mode because
that endpoint has no process identity. Captured game sound still routes to a
connected controller headset, and waveform haptics and microphone permissions
remain independent. Process loopback includes all of the game's render outputs;
games rendering actuator signals as audio may also put those signals in the
capture. If that happens, use System audio on the normal playback endpoint.

Desktop entries, detached or unconfirmed launches, several independent game
process trees, unsupported Windows versions and capture errors send silence
and an actionable session warning. They never switch to system capture.
Choose a direct executable entry or explicitly select System audio to resolve
those cases. Arbitrary multi-application selection and separate volume mixing
are not implemented in this first game-only mode.

## Displays and RTSS

Under **Settings → Display**, select a physical display or choose a virtual display per device/shared by all devices. **Exclusive** switches other monitors off; choose **Extended** to keep the existing desktop active. The primary and isolated variants control where the virtual display sits in that desktop.

Resolution and refresh policies default to `auto` (`dd_resolution_option`, `dd_refresh_rate_option`). Virtual refresh follows the frame-limiting policy unless a manual or device display mode overrides it. To leave a **physical** monitor's resolution, refresh and HDR unchanged, select Disabled for `dd_configuration_option`.

A disconnect can leave the app and its display available for reconnection. Enable `dd_config_revert_on_disconnect` if you want the display configuration restored on disconnect; it defaults to `false`. Without it, a disconnected device's display is kept while its app runs for `dd_paused_virtual_display_timeout_secs` (default `7200` seconds; `0` keeps it until the app closes). A client that sends nothing for `ping_timeout` (default `10000` ms) ends its stream. **Maintenance** provides display restoration and saved-baseline controls.

**Virtual display → Automatic (Windows default)** leaves the choice to the host: one virtual display per device on Windows 11, or a physical monitor on Windows 10. Selecting **One for each device** explicitly also enables that mode on Windows 10. For **Permanent virtual displays**, an empty field preserves the driver's current count; `0` explicitly removes permanent displays at the next host startup. Resetting this setting is therefore different from entering `0`.

Under **Settings → Frame limiting**:

- Keep **Limiter** on Automatic, or select RTSS. Set **RTSS folder** (`rtss_install_path`) only if detection fails; an empty value searches Program Files.
- Rubylight starts RTSS when a limit is needed and restores the previous limit after disconnect. If RTSS requires administrator access, the installed service can use the signed-in administrator's token; otherwise start RTSS with the required access yourself.
- **Frame limit** (`frame_limiter_fps_limit`) defaults to `0`, meaning the stream rate. RTSS preserves fractional rates such as 59.94 FPS.
- **Virtual display refresh** offers 2× (`legacy`), 4× (`enabled`), 1000 Hz (`vrr`) or Off (`disabled`). Turning **Limit every stream** off does not disable the automatic virtual-display limit. Choose **Limiter → None** (`frame_limiter_provider = none`) to disable all limiting.

## Playnite launch focus

When launching a Playnite game or fullscreen menu, Rubylight can bring its window to the front so controller input reaches it. Under **Settings → Library → Advanced → Playnite launch focus**, **Confirm game focus** (`playnite_focus_attempts`, default `3`) sets the number of successful focus confirmations. Rubylight checks once a second, for at most **Stop focusing after** (`playnite_focus_timeout_secs`, default `15` seconds).

If launch keeps interrupting a window you are using on the host, enable **Stop after the first focus confirmation** (`playnite_focus_exit_on_first`, default `false`) or set either numeric limit to `0` to disable focus management. Stopping early lets a launcher or splash screen take focus back; increase the timeout when a slow launch otherwise leaves the game behind another window. Save and launch again to apply these settings.

## Controllers

**Settings → Input → Controller type** controls which virtual controller games see. Standard profiles use the VHF gamepad driver included in the installer. Composite DualSense waveform haptics and native Steam Deck controllers use the optional usbip-win2 driver, bundled in the installer as **Steam Deck and DualSense USB audio and haptics**.

`gamepad = auto` matches each client's controller. PlayStation clients get a DualSense, the only pad with adaptive triggers; it also carries motion, battery, touchpad, lightbar and rumble. A game's trigger effects reach the player only if the client forwards them to a real DualSense. Nintendo clients get a Switch Pro. Other clients get an Xbox Series pad, or a DualSense when they have motion sensors or a touchpad and `motion_as_ds4` or `touchpad_as_ds4` is enabled. Both preferences default to enabled, so a Steam Deck reporting motion gets a DualSense.

`vhf_xbox`, `vhf_xbox_one`, `vhf_ds4`, `vhf_ds5` and `vhf_switch` give every client that pad: `vhf_ds5` a DualSense, `vhf_ds4` a DualShock 4 without adaptive triggers. Logs name the profile for each connected controller.

Earlier releases also offered Xbox 360 (`x360`) and DualShock 4 (`ds4`) through ViGEmBus. Since rc.25 Rubylight changes those settings to `vhf_xbox_one` and `vhf_ds4` when it loads them, in the host configuration, app and device overrides, and configurations imported from Sunshine, Apollo or Vibepollo. An installed ViGEmBus is no longer used and can be uninstalled.

### DualSense waveform haptics

With a waveform-capable client and usbip-win2 installed on the host, Automatic can expose a DualSense with its matching USB audio endpoint. **DualSense with waveform haptics (USB/IP)** (`gamepad = usbip_ds5`) explicitly requires this mode. Games can then send their native actuator waveforms alongside ordinary rumble and adaptive triggers. See [setup and current limitations](dualsense-haptics.md). Available in Rubylight 2.2.15 and later.

### Steam Deck

A Steam Deck can reach the host as a **real Steam Deck controller**: Rubylight serves the Deck's own USB controller (Valve 28de:1205) over USB/IP on the host's loopback address, and [usbip-win2](https://github.com/vadimgrn/usbip-win2) attaches it with its signed driver, so Windows sees a Steam Deck plugged into a USB port. Steam on the host then recognises a Steam Deck and applies the Deck's Steam Input layout, with both trackpads, the gyro, the back grips (L4, R4, L5, R5), the Steam and "…" buttons and rumble. Games see whatever Steam Input makes of it, as on a Deck.

- **Settings → Input → Steam Deck and back grips → Steam Deck controller** (`steam_deck_controller`): **Automatic** (`auto`, the default) attaches a Steam Deck when usbip-win2 is installed and Steam is running on the host when the controller connects; **Steam Deck** (`steam_deck`) attaches one whenever usbip-win2 is installed, even without Steam, where only SDL-based games see it; **Virtual pad** (`virtual_pad`) always uses the virtual DualSense or Xbox pad below.
- **Install USB audio and haptics support on the host.** Rubylight 2.2.15 and later bundle usbip-win2 0.9.8.1 under **Steam Deck and DualSense USB audio and haptics**; select the optional component when convenient to reconnect USB devices. Older packages need usbip-win2 from its [releases](https://github.com/vadimgrn/usbip-win2/releases) (Windows 10 1903 or later). Setup can reconnect USB hubs and require a restart. Rubylight finds `usbip.exe` through its registered installation, `C:\Program Files\USBip`, or `PATH`. Nothing listens beyond `127.0.0.1`.
- If usbip-win2 is missing or the attach fails, the Deck falls back to the virtual pad, and the stream card says why when **Steam Deck** was chosen or an attach failed.

Moonlight passes on the Deck's own controls only when Steam Input is off for Moonlight on the Deck. With it on, Steam on the Deck turns the controls into a virtual pad first: the host still gets a Steam Deck with the buttons, sticks and triggers, but no gyro, trackpads or back grips, because Moonlight never receives them. On the Deck, open Moonlight's controller settings in Steam, choose to disable Steam Input, and reconnect. If the paired device's name says Steam Deck (SteamOS calls it `steamdeck`) and its controller arrives without a gyro, the stream card says this.

Moonlight sends trackpad touches from builds whose SDL reads the Deck's trackpads (SDL 3).

With the virtual pad (no usbip-win2, or **Virtual pad**), Moonlight's Steam controller with gyro gets a DualSense:

- **Gyro and accelerometer** are the pad's motion sensors.
- **Trackpads** share the pad's one touch surface: the left trackpad is its left half and the right trackpad its right half. Clicking the right trackpad is the touchpad click.
- **Back grips** (L4, R4, L5, R5): the virtual pads have no back buttons, so each grip presses what **Settings → Input → Steam Deck and back grips** sets for it: a face button, bumper, a fully pulled trigger, a stick click, Back, Start, Guide, a d-pad direction, the touchpad click or Share/Mute (`back_grip_l4`, `back_grip_r4`, `back_grip_l5`, `back_grip_r5`; values `a`, `b`, `x`, `y`, `lb`, `rb`, `lt`, `rt`, `l3`, `r3`, `back`, `start`, `guide`, `dpad_up`, `dpad_down`, `dpad_left`, `dpad_right`, `touchpad`, `misc`). They default to nothing, and pressing an unmapped grip shows a hint on the stream card. The same settings map an Xbox Elite's paddles (P1 is R4, P3 is L4, P2 is R5, P4 is L5) and a DualSense Edge's back buttons.
- **Steam and "…" buttons** become the PS button and the DualSense's mute button (the touchpad click on a DualShock 4), when Steam on the Deck lets them through to Moonlight.
- **Rumble and battery level** are passed on.

Setting `motion_as_ds4` and `touchpad_as_ds4` both off keeps a Deck's virtual pad an Xbox pad, without its gyro and trackpads.

## MoonDeck

[MoonDeck](https://github.com/FrogTheFrog/moondeck) works with Rubylight. It launches Steam games from the Steam Deck through Moonlight, with [MoonDeck Buddy](https://github.com/FrogTheFrog/moondeck-buddy) managing Steam on the host. You only need one host app for MoonDeck; you do not need to add each Steam game to Rubylight.

1. Install MoonDeck on the Steam Deck and [MoonDeck Buddy on the Windows host](https://github.com/FrogTheFrog/moondeck-buddy/wiki/Buddy-installation-guide). Keep Buddy running, pair it through MoonDeck's settings, and [pair Moonlight with Rubylight](getting-started.md#pair-moonlight-and-start-desktop).
2. In Rubylight's **Library**, add an app with **Name** set to `MoonDeckStream` and **Command** pointing to `"<BuddyInstallDirectory>\bin\MoonDeckStream.exe"`. Replace `<BuddyInstallDirectory>` with Buddy's actual installation folder and keep the quotes around the path.
3. Under the app's **Behavior**, turn off **Keep the stream when a launcher exits within 5 seconds** (`auto-detach`) and save. **Wait for every process the app starts** (`wait-all`) can stay enabled. These correspond to the options in MoonDeck's [host setup guide](https://github.com/FrogTheFrog/moondeck-buddy/wiki/Sunshine-setup).
4. In Buddy's `settings.json`, next to its executable on Windows, set `sunshine_apps_filepath` to the full path of Rubylight's `apps.json`, then restart Buddy. For a standard installed service, the JSON entry is `"sunshine_apps_filepath": "C:\\ProgramData\\Butterpollo\\config\\apps.json"`. For a portable or custom profile, use `apps.json` in the [active profile folder](getting-started.md#bring-an-existing-profile), unless `file_apps` points elsewhere. Buddy's automatic lookup looks for Sunshine and can otherwise find the wrong installation or no app list. See **Sunshine apps filepath** in [Buddy configuration](https://github.com/FrogTheFrog/moondeck-buddy/wiki/Buddy-configuration).
5. Select the Rubylight host in MoonDeck and use `MoonDeckStream` as its host app. The name must match the app saved in Rubylight.

### Controls with MoonDeck

Moonlight carries controller, keyboard and mouse input to Rubylight. MoonDeck launches a separate Steam shortcut for the stream, so configure the **running MoonDeck shortcut's** controller layout on the Deck; settings on the standalone Moonlight shortcut do not configure it.

- **Deck-side layouts.** Keep Steam Input enabled on the Deck for that shortcut and choose a gamepad layout, or edit it for the game. Map L4/R4/L5/R5 to buttons or keys, and map gyro or trackpads to mouse or stick movement as needed. Moonlight sends the resulting inputs; these mappings do not send the raw motion sensors, separate trackpad touches or back-grip identities to the host.
- **Community layouts.** In MoonDeck's **Game Session** settings, enable **Automatic title switch to AppId (MoonDeck app only)** to access the game's community controller layouts. You can also toggle **AppId as game title** in MoonDeck's right-side quick-access panel during a stream. With this enabled, use **Exit MoonDeck** in that panel to stop the session; Steam's left-side exit button does not work. Opening side panels can temporarily lose controller focus. These options and limitations are described in [MoonDeck's Game Session settings](https://github.com/FrogTheFrog/moondeck/blob/main/src/components/gamesessionview/gamesessionview.tsx).
- **External controllers.** MoonDeck's **Game Session → Selected Steam Input option → Disable Steam Input** lets Moonlight see the actual external controller, including supported motion input. MoonDeck documents this option for external controllers; do not assume it disables Steam Input for the Deck's built-in controls.

For **host-side Steam Input with the Deck's own controls**, follow the [Steam Deck controller setup](#steam-deck): install usbip-win2 on the host, use a client that exposes the required controls, and disable client-side Steam Input for the shortcut MoonDeck actually launches. Start Steam on the host before connecting when using **Steam Deck controller → Automatic**. This mode depends on the client sending raw controls; a working button layout alone does not confirm gyro, trackpad or back-grip passthrough.

If controls do nothing, check the paired device's **Controllers** permission under **Devices** and the console's stream warnings. Keyboard and mouse mappings also need their respective device permissions. See [input troubleshooting](troubleshooting.md#a-controller-or-other-input-does-nothing) for permissions and the VHF gamepad driver.

## Settings for one app or device

Edit an app in **Library** or a paired device in **Devices** to set its display, HDR and other overrides. Leave an override unset to inherit the host configuration.

For general configuration overrides, Rubylight applies **host settings → device overrides → app overrides**. Only supported stream, input, display and encoder keys are accepted; host-wide network, identity and path settings cannot be overridden per stream.

Display selection has dedicated rules: a device's explicit virtual-display mode takes priority over the app's mode, and its **display mode** (`WIDTHxHEIGHTxREFRESH`) overrides the host's resolution/refresh policy. This does not change the frame rate requested for the encoded stream.

### Display choice from the client

A client can choose, for one stream, what the PC's displays do. [Nightjar](https://github.com/RamazanKara/nightjar) sets it per PC under **PC settings… → Display on the PC**. The client sends `hostDisplay` with `/launch` or `/resume`:

| `hostDisplay` | What the stream gets |
| --- | --- |
| `exclusive` | A virtual display, and the PC's other displays switch off for the stream. |
| `extended_primary` | A virtual display beside the PC's displays, made the primary display so games open on it. |
| `extended` | A virtual display beside the PC's displays; the primary display stays as it is. |
| `extended_isolated`, `extended_primary_isolated` | The isolated layouts of **Settings → Display**. |
| `physical` | No virtual display; the stream shows the physical display the host and app settings select. |
| missing, `default` or anything else | The host decides from its settings, as for an older client. |

The choice comes before the host, device and app settings for virtual display mode and layout, including a device's **Always use a virtual display**. Two cases still get a virtual display after `physical`: a PC with no active display, and an output that names the virtual display itself. A virtual display needs its driver; without it the stream shows the physical display with a warning, as for any other request. When another stream already holds the display layout, a second stream joins that layout. The layout from before the stream comes back when the stream ends, as with the host's own settings. A display kept for reconnection is reused only when the next launch asks for the same choice, or asks for nothing. Remote Monitor and Remote Input ignore the parameter. Older Rubylight hosts and other hosts ignore it, so Nightjar also sends `virtualDisplay=1` with the virtual choices, which those hosts honour where they support a virtual display.

## Save, reconnect and restart

Select **Save changes** to write the configuration. Edits are not saved merely by changing a control or switching categories. Saved stream settings are read when a new stream starts; an existing stream generally keeps its current configuration.

The console shows **“Saved. Some changes apply after the host restarts.”** Startup settings, including network listeners and advertised codec capabilities, need a restart. Use **Restart now** after disconnecting: restarting the host disconnects active streams.

For file-based configuration, the file is still named `sunshine.conf` for compatibility:

| Installation | Default profile folder |
| --- | --- |
| Windows service | `%ProgramData%\Butterpollo\config` |
| Portable host | Existing `config` folder beside the executable (2.2.6 or later); otherwise `%LocalAppData%\ButterpolloRust\config` |
| Explicit profile | Folder passed to `butterpollo.exe --config-dir <folder>` |

The format is `key = value`, one setting per line. Stop the host before editing files directly, then start it again to load them. Prefer the console for ordinary changes; old or unknown keys can be preserved without being implemented by the Rust host.

## Multiple instances

Give each instance its own profile folder and base port. An explicit `--config-dir` takes precedence over the portable defaults. From the extracted package, run these commands in separate PowerShell windows, one in each intended Windows session:

```powershell
.\butterpollo.exe --config-dir C:\RubylightProfiles\Seat1 --port 48123
.\butterpollo.exe --config-dir C:\RubylightProfiles\Seat2 --port 48323
```

The consoles are at **https://localhost:48124** and **https://localhost:48324**. These base ports also keep the instances' streaming ports separate from each other and from the default installed host. To make the port choice persist when using **Start Rubylight.exe**, save `port = 48123` or `port = 48323` in the corresponding profile's `sunshine.conf`. The launcher accepts `--config-dir`; `--port` is a host option.

For copied portable packages, create a separate `config` folder beside each copy of `butterpollo.exe` and set a different `port` in each folder's `sunshine.conf` before launching. Automatic selection of this adjacent folder requires [2.2.6 or later](../rust/RELEASE_NOTES.md#new-in-226); older packages need an explicit `--config-dir`. Without an adjacent folder or explicit path, copies running as the same account share the per-user default, including SYSTEM's systemprofile location. Start with fresh profiles for independent identities and pair each instance separately; copying a populated profile also copies its identity and paired devices.

For multiseat use, launch each process in its intended Windows session through your multiseat software, using that session's SYSTEM token if required by your setup. The installer continues to manage one Windows service. Separate-profile startup has been smoke-tested with two simultaneous hosts; simultaneous streaming from different Windows users still needs validation.

## Updates

Automatic **checking** is enabled; automatic **installation** is not. Use **Maintenance** to check manually or start an offered update. Enable **Include pre-releases** if you want release candidates.

If you opt into **Install updates automatically**, the installed Windows service waits for one minute without active streams, pending connections, remote-monitor sessions or running host apps. The updater validates the selected installer against the release's size and SHA-256. Portable installations use the release download page instead.

Set `update_check_interval = 0` to disable scheduled checks; manual checks remain available.

## Further reference

- [Features](features.md)
- [Measured performance and testing conditions](../rust/PERFORMANCE.md)
- [Console settings definitions](../rust/web/src/lib/settings-schema.ts), including [video](../rust/web/src/lib/schema/video.ts), [display](../rust/web/src/lib/schema/display.ts) and [general settings](../rust/web/src/lib/schema/basics.ts)
- [Configuration parser and allowed overrides](../rust/core/src/config.rs)
- [Historical C++ configuration reference](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/configuration-cpp.md) — preserved for older installations; its defaults and platform advice do not describe the current Rust host.
