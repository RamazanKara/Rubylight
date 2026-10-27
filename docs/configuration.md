# Configuration

[Docs](README.md) · [Getting started](getting-started.md) · [Troubleshooting](troubleshooting.md)

Configure the host in the Rubylight console, normally at **https://localhost:47990**. Open **Settings**, which groups its controls into General, Streaming, Display & HDR, Audio & controls, Games & automation, and Network & access, and search by a setting's name or configuration key. Resolution, stream frame rate, bitrate and codec are normally chosen in the client.

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
| Encoder — `encoder` | `auto` | AMD AMF for H.264, HEVC and AV1, and PyroWave's own encoder for PyroWave. There is no other encoder and no fallback. |
| AMD usage — `amd_usage` | `ultralowlatency` | AMD's lowest-latency preset. The driver also turns on its internal low-latency mode, and the lowest latency for AV1. |
| Stream preset — `stream_preset` | `balanced` | Balanced is every default in this table; nothing is changed. See [Stream presets](#stream-presets). |
| AMD quality — `amd_quality` | `speed` | Favours low encoding latency. |
| Frame pacing — `frame_pacing` | `arrival` | Encodes arriving frames, limited to the requested stream rate. |
| Use the launch frame rate — `limit_framerate` | `false` | Off, the stream runs at the rate the stream request asks for, as in Vibepollo; a virtual display's refresh and the frame limiter follow it. On, the launch request's rate is encoded, as before. Artemis's Warp frame pacing launches at 60 and asks for 120 or 240. The client's bitrate is the budget either way. |
| Video packet pacing — `pacing_max_bitrate_kbps` | `0` | Route-based: fast wired pacing; confirmed host Wi-Fi uses twice the encoder bitrate with FEC headroom. Positive values set a fixed limit. PyroWave keeps its separate policy. |
| Adaptive packet pacing — `host_adaptive_pacing` | `false` | Experimental opt-in: slower startup, tentative rate increases and loss-based rollback. Does not change picture bitrate or FPS. |
| Adaptive packet pacing for Wi-Fi clients — `wireless_adaptive_pacing` | `true` | Clients that report a Wi-Fi or cellular route (`x-rl-video[0].clientWireless:1`, Nightjar) get the adaptive packet pacing controller on H.264, HEVC and AV1: it starts at twice the encoder bitrate and probes up while the link stays clean. PyroWave is not affected. A manual pacing limit or host bitrate adaptation turns it off. |
| Slow down after video loss — `keyframe_loss_pacing` | `true` | After a keyframe the client could not rebuild, a repeated request for the keyframe it was just sent, or three distinct unrecoverable frames within two seconds (frames the host could not send whole count too), paces video like Wi-Fi. Only loss that lasted is remembered for that device, address and network interface for 24 hours; a lost first keyframe alone is lifted after 10 clean seconds and forgotten. Encoder bitrate is unchanged. Off with a manual pacing limit, either adaptive option, or a stream that adaptive pacing for Wi-Fi clients paces automatically. |
| Lower bitrate when the link cannot carry it — `link_rate_adaptation` | `true` | Cuts the encoder bitrate once, to 85% of what the link delivered, when the client reports loss FEC cannot repair, then probes back up. A healthy link never changes. Off for PyroWave, when the client sets the bitrate, and with `host_adaptive_bitrate`. [Details](#lower-bitrate-when-the-link-cannot-carry-it). |
| Host bitrate adaptation — `host_adaptive_bitrate` | `false` | Experimental loss adaptation for H.264, HEVC and AV1. Requires client feedback and yields to client bitrate control. [Details](#host-bitrate-adaptation). |
| Audio to stream — `audio_capture_mode` | `system` | Captures the selected playback device. Optional [Game only](#game-only-audio) captures a verified game's process tree. |
| HEVC / AV1 support — `hevc_mode`, `av1_mode` | `0` | Automatic capability detection; only working profiles are offered. |
| PyroWave — `pyrowave` | `true` | Offers PyroWave to clients that support it. |
| Virtual display — `virtual_display_mode` | `per_client` on Windows 11 23H2 or later, `disabled` on earlier Windows | A display for each device. Requires the virtual display driver when enabled. |
| Virtual display layout — `virtual_display_layout` | `exclusive` | Turns other displays off while the virtual display is used. A Remote Monitor follows it while it streams: exclusive turns the other displays off and makes it primary, the primary layouts make it primary, and it keeps the place the remote monitor layout gives it. Choose Extended to use a remote monitor beside your own. A layout you pick for a device under Devices overrides this and the host never replaces it. A device without one of its own keeps the layout you leave a stream's display in, such as Windows' "Show only on 2", and uses it from the next stream; set it back to the host setting to forget that. A layout changed through the clients update API (`/api/clients/update`) counts as set by you; send `virtual_display_layout_set_by_user: true` to mark one without changing it. |
| Physical display preparation — `dd_configuration_option` | `verify_only` | Requires the selected monitor to be active; resolution, refresh and HDR policies are separate. |
| Display HDR — `dd_hdr_option` | `auto` | Matches the streamed display's HDR state to the stream. |
| Windows HDR Calibration on virtual displays (Experimental) — `virtual_display_hdr_calibration` | `false` | An HDR stream's new virtual display gets the profile the Windows HDR Calibration app made for your HDR monitor (the primary one first), and gets its own profile back after the stream. Off by default because the profile describes that monitor, not the device you stream to. A profile chosen for the device (`hdr_profile`) comes first; with none found, the host log says so and the stream stays uncalibrated. |
| Limit every stream — `frame_limiter_enable` | `false` | Manual limiting is off; virtual-display automatic limiting can still apply. |
| Limiter — `frame_limiter_provider` | `auto` | Uses RTSS when installed. |
| Virtual display refresh — `frame_limiter_auto_virtual_framegen` | `legacy` | Runs the virtual display at twice the stream rate and enables automatic frame limiting. |
| Host control for a device — `host_control` | off | Per device, in **Devices → device → Custom permissions → Other**. Lets that device change Radeon settings and close the running game through the native API. [Details](#host-control-for-a-device). Only the console grants it, as `true`; pairing, imported Vibepollo or Apollo state and a full permission value never do. Turning it off takes effect on the next request and doesn't end the stream. |
| Sleep PC for a device — `allow_sleep` | off | Experimental. Per device, in **Devices → device → Custom permissions → Other**. Lets that device put this PC to sleep through Vibepollo's `/power-actions/v1` requests while nothing is streaming or running: a single-use token valid for 30 s, then the suspend once the answer is sent and the connection closed. Needs a PC with S1, S2 or S3 sleep, as most desktops have. On a PC with only Modern Standby, Sleep PC is unavailable: Windows has no call to enter Modern Standby, and on such a laptop with hibernate on the request hibernated the PC instead. There the host advertises no Sleep PC, refuses the request, and the console shows why instead of the setting; the host log says so at startup and names the sleep states the PC reports. The console grants it as `true`; a device imported from Vibepollo with `allow_sleep: true` keeps it. |
| Client roaming (Experimental) — `client_roaming` | `true` | A device whose address changes, such as a phone moving from Wi-Fi to mobile data, keeps its stream. The device proves the new address with the stream key first; the host then sends there and drops what arrives from the old address. Only devices that support it (Nightjar) use it; other Moonlight clients stream as before. Off, the host neither offers it nor answers roaming datagrams. Turning it off applies at once; turning it on applies from the next stream. |
| Update checks — `update_check_interval` | `86400` seconds | Checks once a day. |
| Automatic installation — `auto_update` | `false` | Announces available updates; installation is opt-in. |
| Include pre-releases — `notify_pre_releases` | `false` | Release candidates are offered only when enabled. |

These are defaults for **unset** keys, not a list of values you must copy into a configuration file.

Under **Settings → Video → Advanced**, a collapsed **Capture troubleshooting** group holds the capture options that matter only when a capture or encoder problem needs isolating: `capture_poll_interval_us`, `capture_predictive_poll`, `wgc_slot_aligned_publish`, `wgc_direct_encoder_input`, `gpu_compute_conversion` and `idd_push_fence_ring`. **Use the launch frame rate** (`limit_framerate`) and **Follow client size and frame rate changes** (`stream_reconfigure`) are also under Advanced, as they matter only to Artemis's Warp frame pacing and to Rubylight clients. `minimum_fps_target` is **Still-picture resend rate**: how often an unchanged picture is sent again, not a floor on a game's frame rate.

When several bitrate and pacing controllers are on, **Adapt bitrate on the host** (`host_adaptive_bitrate`) wins over **Lower the bitrate when the link cannot carry it** (`link_rate_adaptation`), and a device that sets its own bitrate takes over from both. A manual **Pacing rate** (`pacing_max_bitrate_kbps`) turns off **Adaptive packet pacing** and **Slow down after video loss**; **Adapt bitrate on the host** turns off both of those too, and **Adaptive packet pacing** turns off **Slow down after video loss**.

## Capture and video

**Graphics card** (`adapter_name`) lists all hardware GPUs reported by Windows DXGI, including GPUs with no attached monitor on hybrid systems. **Automatic** uses the GPU that owns the captured display. A GPU appearing in the list does not confirm encoder or codec support; that still depends on its hardware and driver. With a GPU selected, virtual displays are created to render on that GPU, as Apollo and Vibepollo do. WGC can capture a display that another GPU renders; Desktop Duplication cannot, so when it is the capture path in use and the display renders elsewhere (a physical monitor on the integrated GPU, or a driver that ignored the request), the stream captures and encodes on the display's GPU and the stream card says so.

**Keep Capture method on Automatic and compute conversion enabled** for normal use. On supported AMD hardware, the compute path lets capture and colour conversion run alongside graphics work. Unsupported textures can fall back to the graphics path. The internal `wgc_compute_copy` switch also defaults to `true`; it is primarily useful for controlled troubleshooting.

**Frames from Rubylight's display driver.** On a virtual display Rubylight created, Automatic captures from the driver itself: it copies each frame Windows composes into textures it shares with the host, so neither WGC nor Desktop Duplication runs, and on AMD the compute conversion reads those textures in place (`idd_push_zero_copy`, default `true`). To select it explicitly, choose **Settings → Video → Capture method → Direct driver push** (`capture = idd-push`). It applies to Rubylight's own virtual displays and falls back to WGC when unavailable, including on a physical monitor. Choosing WGC or Desktop Duplication explicitly requests those methods. The stream captures with WGC instead, and the stream card says why, when the driver is not installed, when another stream with different capture settings already captures that display, or when the display's frame push failed twice before delivering a picture. A display that failed is tried again once it is recreated or renders on another GPU. Save and end all streams using the display before starting a new session to compare capture methods; simultaneous streams may share an already-open capture.

**GPU fence transport** (`idd_push_fence_ring`, default `true`) is the transport for direct driver push on AMD with compute conversion and in-place capture enabled. It uses D3D12-owned shared textures and GPU fences while the display driver continues using D3D11. Set it to `false` to use the keyed-mutex ring instead. It requires a compatible Rubylight display driver; missing support, failed setup or failed synchronization uses the keyed-mutex ring and shows a warning. A rejected fence transport stays rejected through capture restarts on that display. Recreate the virtual display or restart the host to retry. Save and end all streams using the display before changing it. WGC and Desktop Duplication remain available. This option does not require the new native D3D12 IddCx swap-chain API.

**Downscaling uses an antialias filter automatically.** Supported GPU conversion paths average each output pixel's covered source pixels for reductions up to 8× per axis. This helps when the desktop and stream sizes differ, including fractional reductions. Native resolution, enlargement and aligned 2× reduction keep the existing filter. Larger reductions retain the previous filter to bound GPU work. Existing HDR transfer handling, cursor composition and aspect-ratio handling remain in the same conversion pass.

The installed service runs WGC through a helper in the signed-in user's session. WGC can fall back to DDX when unavailable. Selecting a particular capture method is useful when investigating a problem; check **Logs** to see which path actually opened.

H.264, HEVC and AV1 depend on the encoder and client. For `hevc_mode` and `av1_mode`, the choices are `0` Automatic, `1` Off, `2` SDR only and `3` SDR and HDR. Restart after changing advertised codec support. AMD's encoder makes 4:2:0 only, so stock Moonlight gets 4:4:4 from an AMD host through PyroWave alone. For full chroma over HEVC or AV1, turn on **Experimental full chroma** (`experimental_packed_444`, off by default) under Video → Codecs (an advanced setting), restart the host, and turn on the matching option in Nightjar 0.7.18 or newer. The host then packs each 4:4:4 frame into an ordinary 4:2:0 picture and Nightjar puts it back together; stock Moonlight never sees the packed layout.

For **native HDR**, enable HDR in the client and use an HDR-capable display and encoder path. Leave **Display HDR** on Automatic so Rubylight can set the source display appropriately. **10-bit SDR instead of HDR** (`prefer_sdr_10bit`, default `false`) deliberately keeps the stream in SDR; leave it off when you want HDR. A client can also ask for 10-bit SDR itself: when it negotiates a 10-bit format (`dynamicRangeMode=1`) and sends `hdrMode=0` (on /launch or /resume), Rubylight streams 10-bit SDR and leaves the display in SDR, whatever this option says. Without `hdrMode`, or with `hdrMode=1`, a 10-bit format stays HDR unless `prefer_sdr_10bit` is on.

**HDR dynamic metadata** (`hdr_dynamic_metadata`, default `true`) adds per-frame brightness statistics to HDR streams: HDR10+ (SMPTE ST 2094-40) in HEVC and AV1, and on HEVC also HDR Vivid (CUVA 005.1). Displays that tone-map with HDR10+ can then follow each scene; clients without it ignore the metadata. The statistics target the client's reported peak brightness, else the host display's, else 1000 nits. They come from the compute conversion (`gpu_compute_conversion`), measured on the GPU while the frame encodes, so AMF streams on the D3D11 conversion, packed 4:4:4 and PyroWave carry none; the host log says so once per stream. If the statistics pass fails during a stream, the stream goes on with static HDR10 only and the web UI says why. Static HDR10 metadata is the same either way. The metadata writers are ported in part from Foundation Sunshine. One switch covers both formats: turn it off if a display handles brightness worse with it.

**PyroWave needs a compatible client** and much more bandwidth than conventional codecs. Desktop and game test scenes on AMD give two warning levels. The floor warns about severe detail loss; the recommendation targets clean pictures in those tests. Passing the floor alone does not mean a clean picture.

| Stream | Severe-loss floor | Recommended |
| --- | ---: | ---: |
| 720p60 | 139 Mbps | 277 Mbps |
| 1080p60 | 187 Mbps | 399 Mbps |
| 4K60 | 747 Mbps | 1593 Mbps |

These values are rounded up to whole Mbps. Below 1080p the floor/recommendation are 2.5/5 bits per pixel per frame; at 1080p and above they are 1.5/3.2. The smaller desktop's one-pixel text needed more bits per pixel. Rates scale with pixel count and frame rate within each band: halve them at 30 fps, double them at 120 fps. Other sizes are estimates; actual quality depends on text size, textures and the picture. The console stream card uses red below the floor and amber below the recommendation, and the log distinguishes both. The [October 12, 2026 Rubylight 2.2.9 validation](../rust/PERFORMANCE.md#pyrowave-decoded-pictures-and-quality-budgets) rechecked these budgets on synthetic desktop and game pictures; it measures decoded quality, not sustained network throughput. [Original October 7 method and criteria](../rust/PERFORMANCE.md#october-7-pyrowave-bitrate-from-representative-pictures).

The PyroWave bitrate you set in the client is the stream's total on the wire, as in Vibeshine. The host keeps audio with its parity and 500 kbps for control. With record framing (Nonary's Moonlight, Nightjar, Aurora), each picture then gets what fits after packet overhead and one block of critical FEC, so a rate the client calibrated is never exceeded. The picture therefore gets about 15–20% less than the number you set; set the bitrate to what your network carries, not to the picture rate you want. The 4K60 recommendation needs more than gigabit Ethernet. Client and host limits still apply: stream setup allows up to 2 Gbps, while the client's runtime `/bitrate` endpoint caps changes at 500 Mbps. If the client or network cannot carry the recommended rate, use HEVC or AV1. Enabling PyroWave does not force ordinary Moonlight clients to use it.

**A Rubylight client can change the stream's resolution and frame rate without reconnecting**, for example when a foldable phone opens its inner screen. The encoder restarts at the new size with a keyframe and pacing follows the new rate; the PC's display keeps its mode and the picture is scaled to fit. `stream_reconfigure` (default `true`) turns this off for the host, a device or an app; clients that never ask are unaffected. [Control messages](control-messages.md) describes the request.

**VRR is client-negotiated.** A VRR request can use a 1000 Hz virtual display when automatic virtual refresh is enabled. That is the host's virtual display rate, not a claim that your TV or monitor refreshes at 1000 Hz. Client and display support still matter.

**Packet pacing limits video bursts.** The default remains the existing route-based policy: with `pacing_max_bitrate_kbps = 0`, H.264, HEVC and AV1 use up to 800 Mbps on wired or unknown routes. Confirmed host Wi-Fi uses twice the encoder bitrate, with at least 25% headroom above packetized demand. A known physical Ethernet link limits pacing to 80% of its reported speed. Hyper-V vSwitches use their physical adapter when Windows exposes an unambiguous binding. The host's link speed does not establish the capacity of the client or the rest of the path.

**Repeated video loss slows packet delivery** (`keyframe_loss_pacing = true`). The guard activates when the client reports a keyframe it could not repair, asks again for the keyframe it was just sent, or reports three distinct unrecoverable frames within two seconds. Frames the host could not send whole because the network send buffer was full are loss too: three of them within two seconds, or one keyframe, activate it, named "lost on send" in the log, and the client's report of the same frame is not counted a second time. Reports for several blocks of the same frame count once. Nothing the host did itself counts: a keyframe the host sent on its own (Alt+Tab, a bitrate change, a restarted capture or encoder) is not a keyframe the client "asked again" for, even if the client asked just before it, and requests and ordinary-frame reports within one second after it are set aside. The same goes for the first half second after the stream's first keyframe (a client that has not shown a picture yet asks again within about 100 ms). Keyframes sent to recover from loss open no such window. The log says what set it off (the frame numbers) and what was set aside. Stale ordinary-frame reports, isolated losses and fully repaired frames do not trigger it. The stream is then paced like a confirmed Wi-Fi route for the rest of the session: twice the encoder bitrate, at least 1.25 times wire demand, within the route ceiling. The encoder bitrate does not change. With the default sender thread enabled, loss feedback received before sender selection moves the next output to that thread, avoiding a slow inline send on capture.

Sustained loss is saved with the paired device's authenticated identity, its IP address, the host network interface the route leaves by, and the client's own link speed when it reports one (Nightjar does on wired links), at stream end. Loss that was only the stream's first keyframe, with nothing else seen, holds the slower pace for that stream until it has been clean for 10 seconds, and is not saved; a second latch in the same stream, or more loss while it holds, is sustained. The next eligible session at that address starts at the slower pace, including its first recovery frame; the one after tries the ordinary route pace again. A different host interface (another network adapter; the adapter is told apart, not the network it is joined to) or a different client link starts without history, and records saved by earlier releases are ignored. A completed probe with at least five seconds and 120 sent frames and no new loss clears that route's history. Entries expire 24 hours after observed loss; simply reusing remembered pacing does not extend that lifetime. **Devices → Reset network history** clears the device's stored routes for subsequent streams, without changing a running stream. A different address starts independently. A client that reports its wired link (`x-ss-video[0].pyrowaveLinkMbps`) caps conventional-codec pacing at 4/5 of the slower of the two network ports from the first packet: a 1 GbE client stays at 800 Mbps, a 100 Mbps client gets 80. A hint below the encoder bitrate does not pace the stream under it: the pace is then 1.1 times the bitrate, and the host log carries one warning naming both values. Only the stream's first keyframe, and a keyframe the client asks for within a second of it, count as the start; a keyframe lost later, such as one from an encoder rebuild, is not provisional. The 10 clean seconds are counted from the last loss. Manual pacing, either adaptive controller, disabled loss pacing, and PyroWave do not use this memory.

Adaptive error correction keeps the configured protection for 30 seconds after loss, then decays by one percentage point every five seconds of clean feedback. At the default 20% ceiling it reaches the 5% floor after 100 clean seconds. Recovery frames retain the ceiling, and clients without FEC feedback retain the configured percentage.

The original keyframe guard was measured through a 100 Mbps loopback bottleneck with a 16 or 64 KiB queue: 1440p60 AV1 and HEVC streams at 40 and 60 Mbps went from no frame received whole in 20 seconds to all but the first few. Those historical measurements do not measure the newer repeated-frame trigger or remembered routes. [Measurements](../rust/PERFORMANCE.md#october-14-keyframe-loss-on-a-slow-port).

**Adaptive packet pacing is experimental and off by default** (`host_adaptive_pacing = false`). It is an option for connections with burst-related loss when manual pacing is inconvenient. Enabling it starts each conventional stream at twice the encoder bitrate or 1.25 times wire demand, whichever is higher, within the route ceiling. A suitable fixed limit is simpler when the constrained path is known. Neither policy establishes the cause of an Onn client's reported problem; that device has not been validated.

**Wi-Fi clients get it automatically** (`wireless_adaptive_pacing = true`). The host sees only its own link, so a wired host cannot tell that the client sits behind an access point. A client that reports a Wi-Fi or cellular route to the host with the optional SDP attribute `x-rl-video[0].clientWireless:1` (Nightjar does; absent or `0` means unknown and changes nothing) gets the adaptive controller below on H.264, HEVC and AV1, even though `host_adaptive_pacing` is off. The controller starts at twice the encoder bitrate (at least 1.25 times wire demand), so the first keyframe is no longer sent at up to 800 Mbps. It then probes up in steps of at most 12.5% while the link stays clean, up to the host route's ceiling, and backs off on loss, exactly as for `host_adaptive_pacing`. Keyframe-loss slowdown and its remembered routes are off for such a stream, because the controller owns pacing, and the encoder's keyframe size cap follows the controller's rate as it does for `host_adaptive_pacing`. Where the controller does not own an H.264, HEVC or AV1 stream (`wireless_adaptive_pacing = false` or host adaptive bitrate), the hint instead applies the Wi-Fi route policy from the first frame (twice the encoder bitrate, at least 1.25 times wire demand, no probing), which also gives the smaller keyframe cap of a Wi-Fi host. A manual pacing limit always wins; the hint changes nothing there. For PyroWave the hint only stops the client's wired-link hint from setting the starting rate, so it starts from the demand-based rate instead, and a pace the client calibrated still wins.

During startup, busy video, recent standard client pings and stable nonzero RTT allow tentative increases of up to 12.5%, separated by at least half a second and a feedback-delay guard. A new loss report during a probe restores the preceding rate and switches to slower recovery. There is no timer-only jump to the maximum. Each reconnect starts conservatively again: a client can move between Ethernet and Wi-Fi without changing its identity or the host's route. Startup pacing adds transmission time, especially to a large first keyframe; with a 40 Mbps encoder and continuously healthy traffic, reaching an 800 Mbps ceiling takes about ten seconds.

Outside tentative probes, the optional controller reduces bursts after repeated, validated client FEC reports show substantial loss. It changes packet spacing without lowering encoder bitrate or FPS. A half-second sample needs two distinct failed frames, or multiple recovered frames with at least 2% of sent packets missing. Old and duplicate reports cannot repeatedly penalize a replacement rate. The rate stays at least 25% above current packetized video demand, including FEC, minimum parity, encryption and network headers, unless the route ceiling is already lower. This floor prevents pacing itself from creating a persistent backlog; it cannot make an overloaded path carry the stream.

Recovery needs recent standard client pings, active video traffic, stable RTT and an initial eight-second clean hold. Small rate increases are tentative; a failed probe restores the previous rate and extends the next recovery hold, up to a minute. Paused video, absent standard pings or missing RTT does not authorize a faster rate. Stock Moonlight does not acknowledge every successfully received video frame: control pings establish activity, not video-path capacity. Loss during probing remains possible, and clients without loss reports cannot trigger loss-based rollback. The session API exposes `packet_pacing`, including startup state, and rate changes appear in the host log. The separate **Host adaptive bitrate** option keeps its existing opt-in quality/bandwidth controller and disables this additional burst controller.

Conventional video batches contain at most about one millisecond of traffic at the selected rate, or one datagram if it takes longer. `video_max_batch_size_kb` remains an additional payload limit (16, 32 or 64 KiB; default 64). With `video_send_thread` enabled (the default), adaptive packet pacing reserves the dedicated video sender from the start so pacing waits do not block capture. Large keyframes can still take multiple frame periods. Startup and steady-state timing are reported separately in the measurement record.

`video_slice_delivery` (`off`, `2` or `4`; default `off`) sends an H.264 or HEVC P-frame in that many FEC blocks, each as soon as the encoder's slices for it have left AMF, instead of after the whole frame. It is experimental and AMD-only, and not in the console. Keyframes, recovery frames and AV1 always go out whole, and a stock Moonlight or Nightjar client reassembles the frames unchanged. Turning it on asks AMF for slice output with at least one slice per block and turns intra refresh off. A stream with its own send thread (current or adaptive pacing) streams the same way: the packetizer thread builds each block as the units arrive and the paced sender sends it. If a frame ever passes the byte cap its blocks were planned for, the host logs a warning, counts it in `slice_delivery_overflows`, requests a recovery frame and sends whole frames for the rest of the stream. `BUTTERPOLLO_TEST_SEND_LOSS=percent:seed[:air]` drops video datagrams at random for tests; unset it otherwise.

Set `pacing_max_bitrate_kbps` to a positive value in **kbps** to override automatic burst control, including for PyroWave. For example, `120000` paces at 120 Mbps. Conventional video retains its floor of 110% of the stream bitrate and ceiling of 80% of a known Ethernet link. PyroWave treats the setting, the client's calibrated pace and 95% of either endpoint's reported link as hard ceilings: stream demand cannot raise them. The host warns if the wire budget exceeds the resulting rate. Reconnect after saving. [Measurements and pacing math](../rust/PERFORMANCE.md#october-14-conventional-video-pacing).

PyroWave starts unmeasured and wireless paths at 1.25 times their estimated wire demand (at least 110% of the stream bitrate before that headroom), within the ceilings above. A wired host alone no longer selects line-rate bursts. A client reporting its wired link can retain fast wired pacing. The loss guard and opt-in adaptive packet pacing also apply to PyroWave; neither changes picture bitrate. Calibration and streaming use the same segmented sends, retry handling and batches of at most one millisecond of wire traffic, 64 packets and 65507 payload bytes. Error correction can adapt up to `pyrowave_critical_fec_percentage`; clients without reports keep that configured protection. Record-capable clients protect the coarse picture. Legacy clients use standard whole-frame parity within their four-block limit; large frames may receive less protection and show a warning. Their parity uses some of the existing picture budget; set PyroWave error correction to 0 to retain the unprotected format. Every encoded frame is limited to one nominal frame budget, including the first frame after a pause.

**Leave AMF's low-latency mode and AV1 latency mode on Driver default.** In the [October 12, 2026 Rubylight 2.2.9 property sweep](../rust/PERFORMANCE.md#retained-amf-property-sweeps-october-12), default ultra-low-latency usage already read back internal low latency for H.264/HEVC and AV1 latency mode 3 on the RX 7900 XT. Explicitly forcing the same settings produced similar timings across three runs, not exact equality or a demonstrated speedup. With Low latency or Transcoding usage, single auto/forced pairs changed the readbacks from off to on and showed mean reductions of 0.30-0.31 ms for HEVC and 1.01-1.37 ms for AV1. These are eight-second synthetic 1440p120/20 Mbps component tests, not full-stream latency measurements. The [October 7 audit](../rust/PERFORMANCE.md#october-7-amf-low-latency-mode-and-av1-latency-mode) documents earlier measurements and the RX 9000 HEVC-freeze concern; forcing these settings remains opt-in.

**Adaptive quantization (`amd_vbaq`) is off for H.264 and on for HEVC and AV1 when unset.** In the [October 12, 2026 Rubylight 2.2.9 settings comparison](../rust/PERFORMANCE_WORK.md#current-amf-quality-settings-comparison), explicitly turning H.264 AQ on reduced VMAF in all four cases: by 1.44 and 11.89 points on the two clips at 1440p120/20 Mbps, and by 0.06 and 0.41 at 50 Mbps. Turning AQ off gave mixed HEVC results, while AV1 VMAF rose in all 14 tested cases by 0.0009-0.2301 points in SDR and 0.0051-0.4822 in converted HDR. Those small AV1 gains do not establish a universal visual-quality benefit; the HDR scores also use converted SDR footage. Set AQ to On or Off to compare your own content, or Driver default to leave it to the driver. The [October 8 findings](../rust/PERFORMANCE_WORK.md#amf-settings-per-codec-quality-at-a-given-bitrate) remain a separate historical result.

The [current default-quality results](../rust/PERFORMANCE_WORK.md#current-amf-default-quality) cover 32 cases; the [settings comparison](../rust/PERFORMANCE_WORK.md#current-amf-quality-settings-comparison) adds 116 alternatives for quality preset, rate control, AQ, usage and high-motion boost. Each case encoded and scored exactly 180 frames, with no minimum VMAF pass threshold. CBR increased p99 frame size in every tested case; its score changes depended on codec and content. Ten high-motion-boost requests completed but lacked effective-setting readback, so their application is unverified. Encode-time deltas use earlier baselines rather than randomized matched trials. HDR rows convert SDR footage with 203-nit white and do not establish native-HDR perceptual quality.

**Leave split-frame encoding on Automatic.** AMF can split one HEVC or AV1 frame across a Radeon's two encoder engines (`amd_split_frame`), and the driver decides whether it does. Automatic asks for it only when the GPU has two engines and the driver has it off; On asks whenever there are two engines, and Off disables the property. In the [October 12, 2026 Rubylight 2.2.9 sweep](../rust/PERFORMANCE.md#retained-amf-property-sweeps-october-12), RX 7900 XT readbacks were 1 for Automatic/On and 0 for Off. Identical-input synthetic throughput was similar: for example, two-run means for 4K HEVC were 197.32 / 197.37 / 197.36 FPS respectively. This does not mean every frame had identical timing or prove how the driver used its engines. The sweep completed 708/720 calls; twelve pre-analysis cases failed and remain documented. It did not assess decoded quality. H.264 has no split-frame property; single-engine GPUs get no property write. [Historical October 7 measurements](../rust/PERFORMANCE.md#october-7-amf-split-frame-encoding) remain separate.

**Encoder focus (`encoder_focus`) is Experimental and off unless you choose it.** `static` (Still areas) attaches an AMF ROI map that gives blocks which stayed still for a few frames, such as text and HUDs, more bits than moving ones; `centre` (Still areas and centre) also favors the middle of the screen and the cursor. The stream stays a normal H.264, HEVC or AV1 stream for every client. It runs only on codecs the startup encoder check verified on this GPU (the host log's `encoder focus check:` line); otherwise the stream shows the `encoder_focus` warning and focus stays off. There is no automatic value. App and device overrides work as for the other encoder keys. Design and measurement plan: [docs/encoder-focus-design.md](encoder-focus-design.md).

**AMF automatically sizes recovery frames to packet pacing.** In **Settings → Encoders → AMD AMF**, the advanced controls below use the stream's encoder bitrate and frame rate. Peak bitrate and VBV remain at driver defaults; maximum frame size defaults to `auto` when unset. Existing explicit settings, including `0` and the previous `2` default when saved explicitly, are preserved. Reconnect after saving. Initial values appear in the `AMF encoder settings` log line; each recovery picture reports its submitted cap after runtime adjustments.

| Key | Values | Meaning |
| --- | --- | --- |
| `amd_peak_bitrate_ratio` | `0`, or `1`–`2` | Peak bitrate as a multiple of the stream bitrate; the console offers 1×, 1.5× and 2×. This is not an individual-frame cap. |
| `amd_vbv_buffer_frames` | `0`, or `0.5`–`2` | Rate-control buffer in nominal frame budgets. One budget is bitrate divided by frame rate, including fractional rates. |
| `amd_max_frame_size` | `auto`, `0`, or `1`–`8`; default `auto` | Automatic budgets about two frame periods at the live packet pace, allowing 2–8 nominal frames after FEC/header allowance. Explicit numeric values retain their fixed nominal-frame budgets; `0` leaves the property to the driver. Uses `MaxAUSize` for H.264, `HevcMaxAUSize` for HEVC and `Av1MaxCompressedFrameSize` for AV1. |

Automatic frame caps are applied before the first encoded picture and reconsidered as pace, bitrate or frame period changes. Recovery FEC and conservative packet/encryption overhead are included. Small increases of at most 10% avoid repeated driver writes; decreases apply immediately. The driver can reject or exceed this soft limit, and the two-frame minimum can exceed the intended transmission window on an undersized link. Existing profiles that explicitly saved `2` retain it; choose **Automatic** to use the new policy.

Each recovery picture logs `video recovery frame` with its encoded size, the driver-read cap associated with its submission, actual packetized wire budget, pace, estimated wire time and pacing reason. `sender_work_ms` measures work in the selected sender (including interleaved packetization in inline mode), not receiver decoding or display latency. The counters can be compared with the stream's `fec_unrecoverable_frames` diagnostics. Missing cap readback is left unknown rather than reported as an applied limit.

These are encoder bit budgets, not extra queued frames or packet-pacing settings. A smaller budget trades picture quality for smaller frames. Intra refresh remains client-negotiated for H.264 and HEVC and does not replace an explicit recovery-keyframe request. AV1 never uses it: a lost AV1 frame is recovered with an invalidated reference frame, or a keyframe when `amd_ltr_frames` is `0` ([measurement](../rust/PERFORMANCE.md#october-27-av1-intra-refresh-and-recovery-after-a-lost-frame)). See the [rate-control measurements](../rust/PERFORMANCE.md#october-7-2026-amf-rate-control-and-recovery-keyframes) before changing these controls.

`amd_rc` remains `vbr_latency` by default. A tight frame limit reduces large recovery bursts and the resulting send queue; it can reduce picture quality, especially during recovery or complex scene changes. [The historical 4K119.88 SDR comparison](../rust/PERFORMANCE.md#existing-amf-frame-size-limit-at-the-observed-pace) measured substantially smaller IDRs with a fixed 2× cap at 70.77 Mbps; it does not establish native HDR game quality or the quality of the new Automatic policy. Compare your content and select `0` for the driver's own behavior, or a fixed multiplier for a consistent budget.

Frame-size limits are best-effort: encoded frames can exceed them. If the driver rejects, ignores or clamps a requested cap during setup or a runtime update, the stream continues with its effective limit and reports an `encoder_frame_size` warning. Explicit peak-bitrate and VBV requests still report an AMF setting error if unsupported. Malformed or out-of-range values remain invalid. Explicit positive frame limits scale with host bitrate adjustments; Automatic recomputes from pacing, and `0` adds no frame-cap request even if the driver has a nonzero default. These settings affect AMD AMF only.

## Lower bitrate when the link cannot carry it

`link_rate_adaptation = true` (the default) lowers the encoder bitrate when the network cannot carry the stream, for example 150 Mbps requested on a 100 Mbps link. It acts only on loss that error correction cannot repair, so a link that carries the stream sees no change at all: no bitrate change, no pacing change, and no reaction to round-trip time alone.

- **What counts.** Three unrecoverable frames within two seconds; at least 5% of the packets the host sent reported missing in two consecutive half-second windows (FEC repairs less than that, so 0.5% random loss on Wi-Fi never triggers it); three frames the host could not send whole; or three network-caused keyframe requests from a client that sends no FEC status. A keyframe request caused by the decoder or by the host itself does not count.
- **What it does.** One cut to 85% of the rate the link delivered, never below half the current rate for a first cut. Reports about frames sent before the cut took effect are ignored, so the queue left over from the old rate cannot trigger a second cut. A second cut within two seconds of the first is not held to half (it goes to 85% of the delivered rate, no lower than 1 Mbps of wire rate), so a drop from 80 to 10 Mbps takes two cuts; a stream only a little above its link needs one.
- **Pacing follows.** Packet pacing follows the bitrate, so a cut also lowers the route pacing, and it rises again with each probe step. `keyframe_loss_pacing` is separate and slows pacing on its own evidence.
- **Coming back.** After two seconds the host raises the bitrate in steps of 12.5% (5% close to the rate that failed), and each step is confirmed after two seconds without evidence. A step that fails goes back to the last good rate and the next try waits twice as long, up to eight seconds. When steps keep failing at the same rate (a link with a fixed limit), the wait under that rate doubles up to 30 seconds and probes stay under 90% of it, so a fixed link pays about one failed probe every 45 seconds instead of one every 9. Probes are sent, and steps confirmed, only while the picture moves. After ten seconds at the requested bitrate the stream is back to normal.
- **What you see.** The first cut writes a WARN line to the host log (`the network to this device cannot carry the stream; lowering the video bitrate`, with the evidence and the old and new bitrate), later changes are INFO lines with `link_rate_change`, and the console's stream card shows "Bitrate limited by the network" with the current and requested bitrate while the stream is below the requested rate. The session API has a `link_rate` object (`state`, `ceiling_kbps`, `current_kbps`, `delivered_kbps`, `steps_down`, `probes`) and the five-second `stream timings` line carries the same as `link_*` fields. The `link_rate` object also has `bad_kbps` (the rate that last failed), `encoder_floor_kbps` and `last_change_ms_ago`. The warning gives the lowest bitrate of the episode ("as low as"), the card the current one. If even the lowest bitrate the host uses cannot be carried, the warning says so and tells you to check the network path.
- **An encoder that will not go lower.** Some encoders stay above the bitrate they are asked for on some pictures (H.264 on AMD sends about 14 Mbps on noisy content at 1080p60 and about 5 Mbps on milder content). When the stream stays more than 20% above the new target in two rounds, the host takes what the encoder sends as its floor, stops cutting below it, says so in the warning and on the card, and probes upward from there instead of cutting down to 2 Mbps.
- **When it is off.** `link_rate_adaptation = false`, `host_adaptive_bitrate = true` (which controls the bitrate itself), PyroWave streams, and a session in which the client has set the bitrate through `/bitrate`.

## Host bitrate adaptation

Available in Rubylight 2.2.16 and later.

**Settings → Video → Adapt bitrate on the host (experimental)** enables
`host_adaptive_bitrate = true` for the next connection. It is off by default.
For H.264, HEVC, AV1 and PyroWave, the host reduces the video budget when a Moonlight
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
accounts for its record wire budget or legacy container overhead and respects
the client's measured pace and link ceilings. A successful client `/bitrate` request takes
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

Direct executable launches
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

## Background streaming

Background streaming lets a device play while someone keeps using the PC, for
example a child gaming on a tablet while a parent works at the desk
([#43](https://github.com/RamazanKara/Rubylight/issues/43)). Turn on
**Devices → (device) → Play in the background** for every stream of that
device (`background_stream`), or **Library → (app) → Behavior → Stream in the
background** for one app (`background-stream`). Either one is enough; it applies
from the next launch.

A background stream:

- **Uses its own virtual display, isolated from the PC's monitors.** The
  layout is always `extended_isolated` whatever the host, device, app or
  client asks for: the PC's monitors and primary display stay as they are, and
  the local mouse cannot reach the stream's display. If the virtual display is
  unavailable the launch fails rather than stream the PC's own monitor.
- **Moves the game's windows onto that display without activating them.**
  Rubylight checks once a second and places windows of the launched game's
  process tree there: a borderless or maximized window fills the display, any
  other keeps its size. A window that keeps moving itself back is left alone
  after five attempts, with a warning on the stream card.
- **Accepts only controllers from the device.** Its keyboard, mouse, touch and
  pen would type and click into whatever the PC user has focused, so they are
  off, as is the clipboard. The virtual controllers work as usual.
- **Streams only the game's sound** ([Game-only audio](#game-only-audio)), so the
  PC user's calls and music stay private. The game is still audible on the
  PC's speakers unless you route it to another playback device.
- **Never takes focus for the game, but tells the game it has it.** Windows
  has one keyboard focus per session and it stays with the PC user. Once a
  second Rubylight sends the game's main window the messages Windows sends an
  activated window, so games that mute or slow down while inactive keep
  their sound and frame rate. Total War: Warhammer III, for example, goes
  silent in the background and plays sound again this way. A cursor clip the
  game then sets on the stream's display is released, so the PC's mouse is
  never trapped there. Windows' background throttling (EcoQoS, coarse timers)
  is turned off for the game's processes. Playnite launch focus and Lossless
  Scaling are skipped, and launched programs get
  `SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1` so SDL games read controllers
  without focus.

Games that ask Windows directly which window is in front are not fooled by
the messages, and neither are games using exclusive fullscreen or Steam
Input (which only feeds the focused game). Choose windowed or borderless mode
in the game. Window placement needs a launch Rubylight can identify, as for
game-only audio: a direct executable, a Steam game or a Playnite game. The
Desktop entry and detached commands only get the isolated display.

### Background streaming from a Hyper-V VM

When the device needs keyboard, mouse or focus-dependent games, run Rubylight
inside a Windows 11 virtual machine instead. Hyper-V GPU partitioning (GPU-PV)
shares the host's Radeon with the VM, and AMF encoding works there, so the VM
is an ordinary Rubylight host with its own session, input and focus while
the PC user keeps the physical desktop.

1. On the host (Windows 10/11 Pro or Enterprise), enable Hyper-V and reboot:
   `Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All`.
2. Create a Generation 2 VM with Windows 11, turn off checkpoints and dynamic
   memory, and give it at least 4 cores and 8 GB.
3. With the VM off, add a GPU partition from an elevated PowerShell:

   ```powershell
   $vm = "Rubylight"
   Add-VMGpuPartitionAdapter -VMName $vm
   Set-VMGpuPartitionAdapter -VMName $vm -MinPartitionVRAM 80000000 -MaxPartitionVRAM 100000000 -OptimalPartitionVRAM 100000000 -MinPartitionEncode 80000000 -MaxPartitionEncode 100000000 -OptimalPartitionEncode 100000000 -MinPartitionDecode 80000000 -MaxPartitionDecode 100000000 -OptimalPartitionDecode 100000000 -MinPartitionCompute 80000000 -MaxPartitionCompute 100000000 -OptimalPartitionCompute 100000000
   Set-VM -VMName $vm -GuestControlledCacheTypes $true -LowMemoryMappedIoSpace 1GB -HighMemoryMappedIoSpace 32GB
   ```

4. Copy the host's AMD driver into the VM: the `C:\Windows\System32\DriverStore\FileRepository\u0*.inf_amd64_*`
   folders of the installed Radeon driver go to
   `C:\Windows\System32\HostDriverStore\FileRepository\` in the guest, and
   the `amd*` DLLs from the host's `C:\Windows\System32` (including
   `amfrt64.dll`) go to the guest's `C:\Windows\System32`. Do not install
   Adrenalin in the guest. Repeat this after every host driver update.
5. Start the VM, install Rubylight in it and pair the device with the VM's IP
   address. Use an external virtual switch so the device reaches the VM
   directly. Rubylight's virtual display gives the VM a screen to capture.

Games with anti-cheat that refuses virtual machines will not start in the VM.

## Displays and RTSS

Under **Settings → Display**, select a physical display or choose a virtual display per device/shared by all devices. **Exclusive** switches other monitors off; choose **Extended** to keep the existing desktop active. The primary and isolated variants control where the virtual display sits in that desktop.

Resolution and refresh policies default to `auto` (`dd_resolution_option`, `dd_refresh_rate_option`). Virtual refresh follows the frame-limiting policy unless a manual or device display mode overrides it. To leave a **physical** monitor's resolution, refresh and HDR unchanged, select Disabled for `dd_configuration_option`.

A device's **Display mode** (`display_mode`, `WIDTHxHEIGHTxREFRESH`) sets its virtual display's resolution and refresh while the stream keeps the size and frame rate the client asks for, so a game can render at 1440p on a 4K stream. To pick just the resolution from Windows instead, leave the display mode empty and turn on **Devices → (device) → Remember the resolution I pick** (`remember_display_mode`): change the stream's virtual display resolution in Windows display settings during a stream, and when the device's last stream disconnects Rubylight keeps it (`remembered_resolution`), puts it back if the stream resumes, and creates the display at it from the next stream ([#36](https://github.com/RamazanKara/Rubylight/issues/36)). The Windows display scale you set on that display during the stream is kept the same way (`remembered_scale`, in percent, for example 200) and applied after the display is at its mode, so a 1440p display comes back at 200% too. A **Virtual display scale** set in the host settings (`dd_virtual_display_scale`, a percentage or Recommended) wins: then the configured scale is applied and no scale is kept. A scale Windows refuses is logged as a warning and the stream goes on at the scale Windows shows, and only a scale Windows actually shows is kept. What is kept is the scale the display shows at the end, and Windows derives it from the recommended scale for the resolution, so a resolution change on its own (a game going exclusive fullscreen, say) can also be kept as a changed scale. The scale is put back when a paused stream resumes, so a change made at the PC while it was paused is not kept. The refresh is not kept: the stream's rate always wins, so the refresh follows the stream and **Virtual display refresh** as usual, including VRR's 1000 Hz, and a refresh Windows refuses falls back at the kept resolution. It applies to a device's own virtual display, not a shared one. Nothing is kept from a stream whose resolution Windows did not take, since the display then shows Windows' choice rather than yours. Turning the option off forgets the kept resolution and scale.

A disconnect leaves the app running but restores its displays by default: `dd_config_revert_on_disconnect=true`, after `dd_config_revert_delay` (default `3000` ms). Exclusive layouts always use that delay, including profiles that explicitly turn restoration off, so the physical monitors do not stay disabled while nobody is streaming. Other layouts can still be kept for reconnection by turning restoration off; `dd_paused_virtual_display_timeout_secs` then controls their lifetime (default `7200` seconds; `0` keeps them until the app closes). Restoration waits for the last stream using a layout. If that layout disabled an original display, the original layout is restored even after a Win+P change; other layouts preserve later user changes. A failed restoration remains in the recovery journal and the next stream retains its original baseline. A device remembers eligible layout changes only at its last stream's disconnect; later local changes during the pause do not become its streaming default. A client that sends nothing for `ping_timeout` (default `10000` ms) ends its stream. **Help & diagnostics** provides display restoration and saved-baseline controls.

**Virtual display → Default (by Windows version)** leaves the choice to the host: one virtual display per device on Windows 11 23H2 or later, none on earlier Windows. **Activate a virtual display** (`dd_activate_virtual_display`) is shown while Virtual display is unset or Off, or while it is on. It gives streams a virtual display when the mode is off, and it also overrides a device's or app's own Off. For **Permanent virtual displays**, an empty field preserves the driver's current count; `0` explicitly removes permanent displays at the next host startup. Resetting this setting is therefore different from entering `0`.

Under **Settings → Frame limiting**:

- Keep **Limiter** on Automatic, or select RTSS. Set **RTSS folder** (`rtss_install_path`) only if detection fails; an empty value searches Program Files.
- Rubylight starts RTSS when a limit is needed and restores the previous limit after disconnect. If RTSS requires administrator access, the installed service can use the signed-in administrator's token; otherwise start RTSS with the required access yourself.
- **Frame limit** (`frame_limiter_fps_limit`) defaults to `0`, meaning the stream rate. RTSS preserves fractional rates such as 59.94 FPS.
- **Virtual display refresh** offers 2× (`legacy`), 4× (`enabled`), 1000 Hz (`vrr`) or Off (`disabled`). Turning **Limit every stream** off does not disable the automatic virtual-display limit. Choose **Limiter → None** (`frame_limiter_provider = none`) to disable all limiting.

## Close apps on disconnect

**Settings → Library → Close the app when the last device disconnects** (`terminate_on_pause`, default `false`) closes the running app after its last streaming device leaves. When off, the app stays running so a device can reconnect. Input-only connections do not keep the app running. The setting takes effect the next time an app starts, including apps started before Windows sign-in.

Each app's **Behavior → Close the app when the last device disconnects** offers **Use global default**, **On** and **Off**. Existing explicit choices, including imported choices, still override the global setting; select **Use global default** to make such an app follow it again. Apps without a saved choice follow the global default, including newly added and automatically synced games.

Closing an app runs its normal shutdown and cleanup; unsaved progress may be lost. This setting controls what happens when a device disconnects. Ending the stream when a game exits remains governed by app process tracking, **Keep the stream when a launcher exits within 5 seconds** and **Wait for every process the app starts**.

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
2. In Rubylight's **Games & apps**, add an app with **Name** set to `MoonDeckStream` and **Command** pointing to `"<BuddyInstallDirectory>\bin\MoonDeckStream.exe"`. Replace `<BuddyInstallDirectory>` with Buddy's actual installation folder and keep the quotes around the path.
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

## Couch menu (Experimental)

The couch menu is a host menu drawn into the stream, so any Moonlight client can reach it ([design](couch-menu-design.md)). These settings choose how it opens:

| Setting | Default | Values |
| --- | --- | --- |
| `couch_menu` | `true` | Off means no menu: no chord is watched and native clients are not offered it. |
| `couch_menu_open_with` | `controller,keyboard,touch` | Which ways open it; leave one out to turn that way off. |
| `couch_menu_controller_chord` | `l3+r3` | The buttons one pad holds, joined by `+`: `select`, `l3`, `r3`, `l1`, `r1`, `guide`, `x`, `y`, `paddle1` to `paddle4`, `touchpad`, `misc`. One button alone works, such as `select`. |
| `couch_menu_hold_ms` | `1000` | How long the pad holds the chord, 300 to 3000 ms. |
| `couch_menu_corner_hold_ms` | `1500` | How long a touch, pen or mouse button holds the top-left corner, 500 to 3000 ms. |
| `couch_menu_keyboard_chord` | `ctrl+alt+backspace` | `ctrl+alt+backspace`, `ctrl+alt+home`, `ctrl+alt+end` or `ctrl+alt+insert`. |

The pad must hold exactly the chord's buttons; sticks and triggers do not count, another button cancels the hold, and the same chord closes the menu. The game sees the chord's presses before the menu opens.

Moonlight clients keep some buttons for themselves, so the host refuses a chord that contains any of these:

- Start+Select+L1+R1, which quits the stream;
- Start, because holding it for 750 ms toggles mouse mode;
- Start+Select, which becomes Guide on pads without a Home button;
- Select+L1+R1+X, which shows the statistics;
- Select+Guide and Select+L1, which moonlight-qt and Moonlight Android turn into a clickpad press.

It also refuses A, B and the D-pad directions, because the open menu reads them: holding a chord with A to close the menu would also press the focused row, such as Disconnect.

`guide` is accepted with a warning: many clients hand Guide to their OS (Android TV's home, the Steam button, iOS), so the hold may never reach the host, and where it does, the Guide press reaches Windows too and opens Xbox Game Bar. A chord with `select` gets a warning when `back_button_timeout` (Hold Back to press Guide) is 0 or more and shorter than `couch_menu_hold_ms`: the host presses Guide before the menu opens. Set `back_button_timeout` to -1 or to at least the hold; a device's override of either setting counts too.

The keyboard chords leave out Ctrl+Alt+Shift, which Moonlight keeps for its shortcuts, and letters, which AltGr types on many layouts. The modifiers can be written in any order, so `alt+ctrl+home` is `ctrl+alt+home`. Holding the keys down does not toggle the menu again; let go of the key and press it again.

`couch_menu_open_with` and the four chord and hold settings can be set per device or app in their overrides, so a phone can open the menu by touch and a TV only by controller. A value the host refuses to save is refused with a message; one edited into the file by hand is reported in the host log and its default is used.

The menu offers Back to game, Disconnect (the game keeps running), End session and Close game. Each runs with the device's permissions as the console has them then, the same checks as the native API's session actions; End session and Close game ask for a second press within 5 s. While the menu is open the game gets no input, and everything the pad, keyboard, mouse and touch held is released when it opens. B or Esc on the top page, the chord again, a tap outside the panel or Back to game close it. Nightjar and other native clients open and close it with `POST /api/native/v1/session/couch-menu` when the host lists `session.couch_menu`.

Input-only sessions and the read-back encode path (`wgc_direct_encoder_input = false`) have no picture the host draws on, so the menu stays closed there and the host log says why; so does a stream too small for the panel.

## Stream presets

`stream_preset` names a set of defaults for three AMD encoder settings: Lowest latency (`lowest_latency`), Balanced (`balanced`, the default) and Reference quality (`reference`). A preset only fills the covered settings that nobody set. A value in the settings file, in a device's overrides or in an app's overrides always wins, so you can pick a preset and still change one setting by hand. The preset's values are not written to the file; changing or clearing the preset changes the ones you did not set. Saving a covered setting in the console stores it, even if it equals the preset's value, so it stays when you pick another preset.

| Setting | Lowest latency | Balanced | Reference quality |
| --- | --- | --- | --- |
| `amd_max_frame_size` | `1` (one frame's share of the bitrate) | `auto` | `0` (driver default, no limit) |
| `amd_quality` | `speed` | `speed` | `speed` |
| `amd_preanalysis` | `false` | `false` | `false` |

- **Lowest latency** limits every frame, recovery keyframes included, to one frame's share of the bitrate. In [the October 7, 2026 recovery run](../rust/PERFORMANCE.md#october-7-2026-amf-rate-control-and-recovery-keyframes) a one-frame cap cut the largest 4K40 HEVC recovery frame by 66% with the same steady frame sizes and encode time. It names the cheapest quality preset and pre-analysis off as well, which are the defaults, so it changes nothing there. A value you set for either still applies and adds its measured cost (1.0 ms for HEVC and 12 ms for AV1 per 4K60 frame for the quality preset; 25 ms against 5-6 ms for pre-analysis).
- **Reference quality** removes the size limit on recovery keyframes (`amd_max_frame_size=0`, the driver default). We have not measured a picture quality gain from that, so the preset claims none. What we measured is the cap itself: in the same recovery run a one-frame cap cut the largest 4K40 HEVC recovery frame by 66% and the encoded payload from 64 to 51 Mbps in the 63 Mbps run. Without the cap, recovery keyframes are larger on the wire, so use it on a fast, steady link. That run measured HEVC; H.264 with a one-frame cap was not measured separately (H.264 and AV1 were only checked at a two-frame cap, for compatibility). Pre-analysis, the quality presets and the quality rate-control modes stay off: the measurements show no consistent picture gain for their encode time. Packed 4:4:4 is not part of any preset: on a Radeon 780M (8-bit HEVC) it improved still text and UI above about 5 Mbit/s of actual bitrate, but scrolling at 5 Mbit/s was 0.8 dB worse and the first frame was worse below 20 Mbit/s; it added 1.2 to 2.1 ms of encode time and 0.7 to 1.2 ms of hardware decode, and was worse at 2 Mbit/s, so it stays the separate **Experimental full chroma** choice ([measurements](../rust/PERFORMANCE.md#october-23-420-against-packed-444-on-text-and-ui-and-afmf-capture-b3-b7-measured-on-the-780m)).
- **Why no preset sets `amd_rc`.** Setting it makes the encoder require that mode and fail loudly if it is unavailable. Constant bitrate is not a quality setting either: in the [AMF quality sweep](../rust/PERFORMANCE_WORK.md#amf-settings-per-codec-quality-at-a-given-bitrate) it scored slightly higher on the first clip only because it spends more of the budget, the 99th-percentile frame grew by 30-67%, and the 48 HDR encodes found no reason to change the rate control. Rate control stays at the default in every preset.

The console shows the selector under **Streaming** and, below it, which covered settings have a value of their own ("Custom"), including one equal to the preset's. Settings the preset covers show its values as their defaults, and Reset returns to them. A name the host does not know still loads, and the host uses Balanced; the console shows the stored name with an error instead of showing Balanced.

A device or an app can name its own preset with `stream_preset` in its overrides (`config_overrides` for a device, `config-overrides` for an app). The most specific name wins (app, then device, then host), and it fills only the three settings that none of the three layers set: an explicit value of the host, the device or the app is never replaced by a preset. The host refuses to save a device or app preset name it does not know, or one that is not text. The device editor has a **Stream preset** control for the device's own name; the app editor sets it through the app's overrides.

## Radeon settings per game

An app can ask for Radeon driver settings while it runs. In **Games & apps**, the **Radeon** panel of the app editor picks a profile:

| Profile | Writes |
|---|---|
| Competitive | Radeon Anti-Lag on |
| Smoothness | Frame Rate Target Control on, at the stream's frame rate |
| Quality | Anisotropic filtering on and the driver's "enhance application settings" anti-aliasing |
| Custom | Up to 24 settings you list (ids as in `GET /api/native/v1/radeon`; a feature's `.enabled` toggle goes first) |

The table is `rust/core/src/radeon_profiles.json`. The app stores the choice as two top-level keys of the app, `radeon-profile` and, for Custom only, `radeon-settings` (an object of setting ids and values, such as `{ "anti_lag.enabled": true }`). The host refuses to save an unknown profile, `radeon-settings` with any profile but Custom (or Custom with none), an id that is not lower-case words joined by dots, a value that is not a boolean, a whole number or a short text, or more than 24 custom settings. At launch the host applies the settings through the Radeon control path of the Session API (validated, journaled, read back, undone on failure), and restores them with the other Radeon changes when the session ends or the host restarts after a crash. The app's config is the administrator's consent, so the device needs no Host control for its game's profile; the device's own Radeon writes (the Session API's panel) still do. The settings are written on a thread of their own, so a launch never waits for the driver, and a game launched from another device first has the earlier device's settings put back, so the new profile's originals are the PC's own. Starting another game on the same device puts back only the settings the earlier profile wrote, not what the device changed itself; Disconnect keeps the profile, and it goes with the session's end (or a launch that never connects). A setting the GPU does not list is refused before anything is written, a profile that follows the stream's frame rate is refused when the launch states none, and both are reported in the session's `radeon_profile.message` and in the host log; the game still starts.

## Settings for one app or device

Edit an app in **Games & apps** or a paired device in **Devices** to set its display, HDR and other overrides. Leave an override unset to inherit the host configuration.

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

The choice comes before the host, device and app settings for virtual display mode and layout, including a device's **Always use a virtual display**. `virtualDisplay=0`, which Apollo-compatible clients send whenever their own virtual display option is off, is not a choice and follows the host settings, as an omitted `virtualDisplay` does. Two cases still get a virtual display after `physical`: a PC with no active display, and an output that names the virtual display itself. A virtual display needs its driver; without it the stream shows the physical display with a warning, as for any other request. When another stream already holds the display layout, a second stream joins that layout. The layout from before the stream comes back when the stream ends, as with the host's own settings. A display kept for reconnection is reused only when the next launch asks for the same choice, or asks for nothing. Remote Monitor and Remote Input ignore the parameter. Older Rubylight hosts and other hosts ignore it, so Nightjar also sends `virtualDisplay=1` with the virtual choices, which those hosts honour where they support a virtual display.

## Host control, session actions and Radeon settings

These are new in 3.0. A paired client such as [Nightjar](https://github.com/RamazanKara/nightjar) 0.8.0 or newer uses them through the [native API](native-api.md). Stock Moonlight and Artemis do not use them and keep working as before.

### Host control for a device

**Host control** (`host_control`) lets one paired device close games and change Radeon settings. It is not part of the other permissions. Open **Devices**, open the device, then under **Permissions → Custom permissions → Other** tick **Host control: change Radeon settings and close games from this device** and save.

- It is off for every device, including new pairings and devices imported from Apollo or Vibepollo. Only the console turns it on; nothing on the streaming port can.
- Turning it off applies on the device's next request. It does not end a running stream.
- A device also needs the **List apps**, **View streams** and **Launch apps** permissions for what it does. Reading the Radeon state needs View or Launch only.

### Session actions

A session has a state the host keeps: idle, preparing, launching, streaming, suspended or stopping. A client with the right permissions can end it in three ways:

| Action | Stream | Game | Virtual display | Radeon changes |
| --- | --- | --- | --- | --- |
| **Disconnect** | Stops now. The session is suspended for the disconnect grace period and can be resumed. | Keeps running | Kept for the grace period | Kept |
| **End session** | Ends. | Keeps running | Released | Put back |
| **Close game** | Ends. | Quits | Released | Put back |

Disconnect and End session need the device to own the session. **Close game** also needs host control. A repeated or racing request is answered once, not doubled. Other devices' streams are not touched. If the grace period runs out, the host ends the session as End session does. If the host stops in the middle of a session, it reports that at the next start. Stock Moonlight's Quit still stops every stream and closes the game, as before. The grace period is `dd_config_revert_delay` when displays are restored on disconnect, and `dd_paused_virtual_display_timeout_secs` otherwise ([displays](#displays-and-rtss)). The states, transitions and errors are in [native-api.md](native-api.md#session).

### Radeon settings with restore

A device with host control that owns a running or suspended session can read and change Radeon settings of the GPU the host encodes on, for example Anti-Lag, Radeon Chill, Radeon Boost, Image Sharpening, Enhanced Sync, a frame rate target, vertical sync, AFMF and RSR sharpness. The host reads them through AMD's ADLX interface and offers a setting only if its write was proven on the installed driver. Each setting is in one of five states, and a client shows only what it can do:

| State | Meaning |
| --- | --- |
| Configurable | The host can change it. A write to this driver was proven on real hardware. |
| Read only | The host can read it but not change it. |
| Needs setup in AMD Software | Turn it on once in AMD Software: Adrenalin Edition first. |
| Not supported | This GPU or driver does not offer it. |
| Temporarily unavailable | The driver is busy or ADLX did not load. Try again later. |

Every change is written to a journal on disk before it is applied, and put back when:

- the session ends, the game is closed or the disconnect grace period runs out;
- the device is unpaired or disabled, or the host shuts down;
- the host starts again after a crash.

It is not put back at Disconnect: a suspended session keeps its settings until it resumes or ends. If you changed a setting on the PC in the meantime, your change stays and the host forgets its own. A restore that fails is logged and retried at the next start. Which settings each driver accepted is recorded for the RX 7900 XT in the [hardware matrix](hardware-matrix.md). The request and response shapes are in [native-api.md](native-api.md#radeon-state-and-writes).

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

## WebRTC server (Experimental)

Off by default. With **WebRTC server (Experimental)** (`webrtc_server`) on, Rubylight listens for TVs and browsers that speak the Moonlight WebRTC protocol (version 2). They can find the PC, pair, sign in, list its apps and stream. [WebRTC server](webrtc.md) has the setup, what streams and the protocol extensions for client authors.

| Key | Default | What it means |
| --- | --- | --- |
| `webrtc_server` | `false` | Turns the server on. Needs a restart. |
| `webrtc_port` | `8000` | TCP port of the WebSocket, announced in the discovery reply. 1024 to 65535. Discovery itself always listens on UDP 8000, where TVs send their broadcast. |
| `webrtc_media_port` | `8001` | UDP port for WebRTC video, audio and input. 1024 to 65535. |

- Pairing: the TV shows a four-digit PIN and the request appears on the console's PIN page next to Moonlight's. Enter the PIN there. The TV then holds a client ID and a secret; the secret is stored in `sunshine_state.json` and is never shown in the console. Paired TVs are listed on **Devices** with a WebRTC badge, where you can disable or unpair them. A disabled or unpaired TV is disconnected at once. At most 32 TVs stay paired; the oldest go first.
- Signalling is plain `ws://`, not encrypted. Signing in keeps other devices from using or interrupting a stream; it does not hide what is sent on the network. Only peers on the local network are served: loopback, private (RFC 1918), link-local, unique-local IPv6 and carrier-grade NAT (100.64.0.0/10) addresses. Anything else is closed before its first message and logged. Do not forward these ports on your router.
- At most 8 connections may sit unauthenticated at once (the oldest is closed for a ninth) and 32 paired devices connected.
- If another program holds the port, the console shows a `webrtc_server` warning, the log has the error and the server does not run. The port never moves on its own.
- `--bind` applies to both the WebSocket and the discovery socket. The installer's Windows Firewall rule allows `butterpollo.exe` inbound on every port, so TCP 8000, UDP 8000 and UDP 8001 (or the ports you chose) need no extra rule.

## File transfer (Experimental)

Off by default. With **File transfer (Experimental)** (`file_transfer`) on, paired devices can send files to this PC and download them again over the streaming HTTPS port ([Native API, Files](native-api.md#files)). Each device also needs **Upload files** or **Download files** under its permissions on **Devices**; no device has them until you tick them, and **Full control** leaves them as they are. Files are written as the Windows user who is signed in; with nobody signed in, transfers are refused.

| Key | Default | What it means |
| --- | --- | --- |
| `file_transfer` | `false` | Turns file transfer on. Applies to the next request. |
| `file_transfer_dir` | empty | The folder uploads go to and downloads come from, as an absolute path. Empty means `Downloads\Rubylight` of the signed-in user. |
| `file_transfer_max_mb` | `4096` | The largest upload in MiB, refused before it starts. |
| `file_transfer_stream_mbps` | `50` | While a stream runs or starts, all uploads and downloads together move at this many Mbit/s. `0` lets them run at full speed. Applies to running transfers within a second. |

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

Automatic **checking** is enabled; automatic **installation** is not. Use **Help & diagnostics → Updates** to check manually or start an offered update. Enable **Include pre-releases** if you want release candidates.

If you opt into **Install updates automatically**, the installed Windows service waits for one minute without active streams, pending connections, remote-monitor sessions or running host apps. The updater validates the selected installer against the release's size and SHA-256. Portable installations use the release download page instead.

Set `update_check_interval = 0` to disable scheduled checks; manual checks remain available.

## Further reference

- [Features](features.md)
- [Measured performance and testing conditions](../rust/PERFORMANCE.md)
- [Console settings definitions](../rust/web/src/lib/settings-schema.ts), including [video](../rust/web/src/lib/schema/video.ts), [display](../rust/web/src/lib/schema/display.ts) and [general settings](../rust/web/src/lib/schema/basics.ts)
- [Configuration parser and allowed overrides](../rust/core/src/config.rs)
- [Historical C++ configuration reference](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/configuration-cpp.md) — preserved for older installations; its defaults and platform advice do not describe the current Rust host.
