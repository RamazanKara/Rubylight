# Configuration

[Documentation](README.md) · [Getting started](getting-started.md) · [Troubleshooting](troubleshooting.md)

Rubylight is configured in its web console, at **https://localhost:47990** on the host PC. The client chooses the resolution, frame rate, bitrate, codec and HDR of each stream; the host's settings decide how it captures, encodes, paces and delivers what the client asks for. The defaults suit a typical setup, and upgrades keep the values you have saved.

This guide covers each feature's settings by topic. [All settings](#all-settings) at the end lists every setting in the console with its configuration key and default.

| Area | Sections |
| --- | --- |
| Basics | [How configuration works](#how-configuration-works) · [Defaults and why](#defaults-and-why) · [Stream presets](#stream-presets) |
| Picture and displays | [Capture and video](#capture-and-video) ([capture](#graphics-card-and-capture-method) · [codecs and HDR](#codecs-full-chroma-and-hdr) · [PyroWave](#pyrowave) · [packet pacing](#packet-pacing) · [AMD AMF](#amd-amf-encoder-settings) · [encoder focus](#encoder-focus) · [NVIDIA NVENC](#nvidia-nvenc)) · [Displays and RTSS](#displays-and-rtss) |
| Network | [Lower bitrate when the link cannot carry it](#lower-bitrate-when-the-link-cannot-carry-it) · [Host bitrate adaptation](#host-bitrate-adaptation) · [Error correction chosen by the device](#error-correction-chosen-by-the-device) · [Roaming](#roaming) |
| Sound | [Game-only audio](#game-only-audio) · [PCM and Dolby Digital audio](#pcm-and-dolby-digital-audio) · [Surround, night mode and headphone surround](#surround-night-mode-and-headphone-surround) |
| Input | [Keyboard, touch, pen and cursor](#keyboard-touch-pen-and-cursor) · [Controllers](#controllers) · [MoonDeck](#moondeck) · [Couch menu](#couch-menu) |
| Games | [Close apps on disconnect](#close-apps-on-disconnect) · [When a Playnite game closes](#when-a-playnite-game-closes) · [Playnite launch focus](#playnite-launch-focus) · [Radeon settings per game](#radeon-settings-per-game) · [Managed game files](#managed-game-files) · [Game settings per device](#game-settings-per-device) · [Neural Rendering](#neural-rendering) · [Background streaming](#background-streaming) |
| Apps and devices | [Settings for one app or device](#settings-for-one-app-or-device) · [Host control, session actions and Radeon settings](#host-control-session-actions-and-radeon-settings) |
| Sharing and browsers | [Clipboard, files and USB](#clipboard-files-and-usb) · [File transfer](#file-transfer) · [Shared folders](#shared-folders) · [WebRTC server](#webrtc-server) |
| Running the host | [Multiple instances](#multiple-instances) · [Updates](#updates) · [Settings backup and restore](#settings-backup-and-restore) |
| Reference | [All settings](#all-settings) · [Further reference](#further-reference) |

## How configuration works

The **Settings** page shows the few settings most people change, in four sections named like the first four tabs of All settings: **This PC** (its name, **Wake this PC from a device**, and **Updates** with **Install updates automatically**, **Check now** and the release status), **Picture** (stream preset, virtual display, and what your monitors do while you stream), **Sound & controls** (stream audio, the device microphone and controllers) and **Games** (closing the app when the last device disconnects). Each section's **More settings** opens its tab in All settings, and the search field at the top finds every other setting.

**All settings**, under **Advanced** in the sidebar, has every setting in six tabs: **This PC**, **Picture**, **Sound & controls**, **Games**, **Network** and **Advanced**. Each tab lists its settings in groups, the ones people look for first at the top. Groups that matter only for troubleshooting (Capture troubleshooting, AMD AMF, NVIDIA NVENC, Mode substitutions, RTSS, Key remapping, Ports and addresses, Console, Debug switches) come last and start collapsed, and **Show config keys** shows each setting's configuration key. **Advanced** also holds **Sign-in and security**: the signed-in browsers, **Change password**, and links to **API tokens** and **Webhooks**. Paths in this guide name the page, the tab, usually the group, and the setting, such as **All settings → Picture → Frame limiter → Frame limit**.

**Help**, the last page in the sidebar, has connection help for devices, **Log analysis** and the support bundle. **Troubleshoot**, under **Advanced** in the sidebar, restarts or quits Rubylight and holds the saved display layout, the Windows session after Remote Desktop, the Nucleus Co-op status and crash reports. **Ctrl+K** (or **Search** in the sidebar) finds pages, games, devices, settings and actions such as **Pair a device** from anywhere in the console.

A game in **Games** and a device in **Devices** can override many of these settings for themselves; see [Settings for one app or device](#settings-for-one-app-or-device). Resolution, frame rate, bitrate, codec and HDR are requested by the client for each stream.

### Find and review settings

Search includes settings hidden by another choice, advanced options and configuration keys. Each hidden result explains when it applies. For example, search `amd_quality` to change AMD quality while keeping **Encoder → Automatic**; the value applies whenever Automatic selects AMF. Changing the encoder is not required just to find its options.

Use **Review changes** to see pending edits across all tabs, including settings hidden after another edit. If saving is blocked by an invalid value, **Review errors** opens the fields to correct. Switching tabs or clearing the search keeps your edits. **Reset to default** removes the saved override when you save; **Undo** restores the value from before the edit.

### Save, reconnect and restart

Select **Save changes** to write the configuration. Changing a control or switching tabs does not save it. Saved stream settings are read when a new stream starts; an existing stream generally keeps its current configuration.

The console shows **“Saved. Some changes apply after the host restarts.”** Startup settings, including network listeners and advertised codec capabilities, need a restart. Use **Restart now** after disconnecting: restarting the host disconnects active streams.

For file-based configuration, the file is still named `sunshine.conf` for compatibility:

| Installation | Default profile folder |
| --- | --- |
| Windows service | `%ProgramData%\Butterpollo\config` |
| Portable host | A `config` folder beside the executable if there is one, otherwise `%LocalAppData%\ButterpolloRust\config` |
| Explicit profile | Folder passed to `butterpollo.exe --config-dir <folder>` |

The format is `key = value`, one setting per line. Stop the host before editing files directly, then start it again to load them. Prefer the console for ordinary changes; old or unknown keys are kept in the file even when the host does not use them.

### Settings outside the console

These keys are read from the configuration file (`sunshine.conf` in the profile folder) but have no field in the console. The audio, couch menu and `absolute_left_release_delay` keys can also be set in device and app overrides:

| Key | Default | What it does |
| --- | --- | --- |
| `audio_night_mode`, `audio_headphone_surround` | `false` | See [Surround, night mode and headphone surround](#surround-night-mode-and-headphone-surround). |
| `couch_menu` and the `couch_menu_*` keys | `true`, and the values under [Couch menu](#couch-menu) | How the couch menu opens. |
| `video_slice_delivery` | `auto` | Sends H.264 and HEVC P-frames in 2 or 4 FEC blocks as their slices leave the encoder; `auto` uses 4 for pictures of 4K size. See [Capture and video](#capture-and-video). |
| `webrtc_pacing`, `webrtc_rtx` | `true` | Send pacing and retransmission for the WebRTC server. |
| `webrtc_loss_permille` | `0` | Drops WebRTC video packets on purpose, in thousandths, for tests. Leave it at 0. |
| `gamepad_feedback_fast_poll` | `false` | After a game's rumble, lights or trigger effects, polls the virtual pads every 2 ms instead of 8 ms for 250 ms, so the next change reaches the device sooner. On a VHF pad it cut the median from about 4.6 to 1.6 ms, but the gamepad thread used about twice the CPU while rumble flowed ([November 8](../rust/PERFORMANCE.md#november-8-gamepad-feedback-touch-and-input-under-load)); leave it off. |
| `gamepad_driver_host_priority` | `false` | While a VHF pad is plugged in, runs the Windows process that hosts the gamepad driver (WUDFHost) at high priority, so a busy game delays controller input less. It changes another process, and other drivers Windows hosts in the same one run raised too; the host log names them. |
| `amd_output_prediction` | `false` | AMF skips an output query while its oldest frame cannot be done yet, judged by the fastest encode it has returned, instead of waiting up to 1 ms in it. Measured with no consistent gain under GPU load; leave it off. |
| `amd_output_thread` | `false` | AMF's output query runs on a thread of its own, so the stream thread never waits in it. Overrides `amd_output_prediction`. Under GPU load it shortened the worst frames' delays but made the typical frame about 1 ms later and used more CPU; leave it off. |
| `nvenc_output_event` | `true` | NVENC on a D3D12 device wakes the stream as each frame is written instead of on its next 100 µs output poll. `false` goes back to the poll alone. |
| `compute_queue_realtime` | `false` | Runs the compute queue that copies and converts captured frames (`gpu_compute_conversion`) at real-time GPU priority. |
| `absolute_left_release_delay` | `10` ms | See [Keyboard, touch, pen and cursor](#keyboard-touch-pen-and-cursor). |
| `file_state`, `file_apps`, `vibeshine_file_state`, `credentials_file`, `cert`, `pkey` | `sunshine_state.json`, `apps.json`, `vibeshine_state.json`, the console sign-in file, `credentials/cacert.pem`, `credentials/cakey.pem` | Where the profile's paired devices, apps, app ids, console sign-in, certificate and private key are kept. A relative path is inside the profile folder. Leave them unset unless you share files with another installation. |

Webhooks have their own console page; see [Webhooks](webhooks.md).

## Defaults and why

<a id="defaults-in-300"></a><a id="defaults-worth-knowing"></a>

Each default below is what a fresh install uses, checked against the host code. The set was chosen for 3.0.0. The stream preset **Balanced** is exactly this set; nothing in the table needs changing for a normal setup.

| Setting — key | Default | Why |
| --- | --- | --- |
| **Capture and encoding** | | |
| Capture method — `capture` | `auto` | On a virtual display Rubylight created, each frame comes straight from Rubylight's display driver, with no capture copy. Elsewhere Windows Graphics Capture (WGC), with Desktop Duplication (DDX) when WGC is unavailable. |
| Copy and convert on a compute queue — `gpu_compute_conversion` | `true` | Copy and colour conversion run on a compute queue beside the game instead of waiting behind its graphics work ([October 4](../rust/PERFORMANCE.md#october-4-capture-copies-and-conversion-beside-a-game)). |
| Encode from GPU memory — `wgc_direct_encoder_input` | `true` | Captured frames stay on the GPU; nothing goes through system memory. |
| Encoder — `encoder` | `auto` | The graphics card's own encoder for H.264, HEVC and AV1 (AMD AMF on Radeon, NVIDIA NVENC on GeForce), and PyroWave's own encoder for PyroWave. There is no software encoder and no fallback, so a failure is reported instead of hidden. |
| Stream preset — `stream_preset` | `balanced` | Balanced is every default in this table. See [Stream presets](#stream-presets). |
| Usage — `amd_usage` | `ultralowlatency` | AMD's lowest-latency profile; the driver also turns on its internal low-latency mode, and the lowest latency for AV1. The other profiles queue frames. |
| Quality preset — `amd_quality` | `speed` | The lowest encode time. With these two defaults the encode stage of a 1080p60 AV1 stream took a median 2.26 ms, and the whole path from capture to the client's present 5.89 ms, on loopback ([October 26](../rust/PERFORMANCE.md#october-26-300-rc2-against-the-published-numbers)). |
| Long-term reference frames — `amd_ltr_frames` | `4` | A device that lost frames recovers from a reference it still has, on H.264, HEVC and AV1, and the host sends a keyframe only where that would be smaller. Over the measured gaps that sent 0.86 to 0.92 of the keyframe bytes ([October 28](../rust/PERFORMANCE.md#october-28-ltr-recovery-on-h264-and-hevc-with-header-rewrites-on-by-default)). One device can set its own value under **Devices** → device → **Encoder overrides**, such as Off for a decoder that freezes on them. |
| Maximum frame size — `amd_max_frame_size` | `auto` | Keeps every frame inside what error correction can still protect, and encodes a scene cut that comes out too large again as a keyframe. Lost scene cuts went from 16 and 15 to 0 and 0, and freezes from 15 to 0 ([October 28](../rust/PERFORMANCE.md#october-28-oversize-scene-cuts-encoded-again-as-keyframes)). |
| Encoder focus — `encoder_focus` | `off` | Favouring still areas makes scene cuts larger, so it is a choice for text-heavy desktops, not a default. See [Encoder focus](#encoder-focus). |
| HEVC / AV1 support — `hevc_mode`, `av1_mode` | `0` (Automatic) | Profiles the encoder fails at host start are never offered. |
| PyroWave — `pyrowave` | `true` | Offered only to devices that ask for it; others never see it. |
| **Frame rate and pacing** | | |
| Frame pacing — `frame_pacing` | `arrival` | Each new frame is encoded as soon as it is captured, limited to the requested stream rate. |
| Frame Warp — `frame_warp` | `true` | A Nightjar device that turns Frame Warp on shows camera turns on its next refresh. The host does the work only for a device that asks. See [Frame Warp](frame-warp.md). |
| Use the launch frame rate — `limit_framerate` | `false` | The stream runs at the rate the stream request asks for, which is what Artemis's Warp frame pacing expects. |
| Pacing rate — `pacing_max_bitrate_kbps` | `0` (automatic) | Fast wired pacing, twice the encoder bitrate on a host Wi-Fi route, and a device's reported link speed caps it. |
| Adaptive packet pacing — `host_adaptive_pacing` | `false` | Starts slower and probes; useful only on a link with burst loss that a fixed rate cannot fix. |
| Adaptive packet pacing for Wi-Fi clients — `wireless_adaptive_pacing` | `true` | A device that says it is on Wi-Fi or cellular gets that controller from the first frame. On a real laptop over Wi-Fi this cut unrecoverable frames from 61 to 15.7 ([October 25](../rust/PERFORMANCE.md#october-25-wi-fi-pacing-hint-on-a-real-laptop)). |
| Slow down after video loss — `keyframe_loss_pacing` | `true` | Only loss that lasts slows packet delivery; the bitrate stays. |
| Lower the bitrate when the link cannot carry it — `link_rate_adaptation` | `true` | Always on. Acts only on loss error correction cannot repair, so a healthy link never changes. See [Lower bitrate when the link cannot carry it](#lower-bitrate-when-the-link-cannot-carry-it). |
| Adapt to network congestion — `congestion_control` | `false` | Off until its A/B on our Wi-Fi devices shows lower latency without fewer frames or more drops. See [Adapt to network congestion](#adapt-to-network-congestion). |
| Adapt bitrate on this PC — `host_adaptive_bitrate` | `false` | Devices that control their own bitrate do it better from their side; this is for clients that do not. |
| Most error correction a device may ask for — `fec_max_percentage` | `50` % | A device that chooses its own error correction may go up to half the video packets. In the October 27 runs this cut Wi-Fi loss and freezes against the host's own share ([October 27](../rust/PERFORMANCE.md#october-27-client-error-correction-and-a-link-test-before-the-first-bitrate-nuc)). |
| Prioritize stream traffic (DSCP) — `dscp_tagging` | `true` | Marked video and audio go first on Wi-Fi; turn it off for a router that mishandles marked packets. |
| Bandwidth test — `bandwidth_probe` | `true` | Lets a paired device time a short download before it picks a bitrate. The device decides whether to run it. |
| Virtual display refresh — `frame_limiter_auto_virtual_framegen` | `legacy` | The virtual display runs at twice the stream rate, so each game frame is captured sooner after it is drawn, and automatic frame limiting applies. |
| Limit every stream — `frame_limiter_enable` | `false` | Physical monitors keep the game's own frame rate; virtual displays are limited automatically. |
| Limiter — `frame_limiter_provider` | `auto` | RTSS when it is installed. |
| **Displays and HDR** | | |
| Virtual display — `virtual_display_mode` | `per_client` on Windows 11 23H2 or later, `disabled` on earlier Windows | Each device gets a display at its own resolution and refresh; only Stream here too shares the display the running game is on. Earlier Windows lacks the driver interface the virtual display needs. |
| Virtual display layout — `virtual_display_layout` | `exclusive` | The other monitors turn off while you stream, so games open on the streamed display. See [Displays and RTSS](#displays-and-rtss). |
| Monitor setup — `dd_configuration_option` | `verify_only` | A physical monitor is only checked to be on, and no monitor is switched on or off. Its resolution, refresh rate and HDR still follow their settings; only `disabled` leaves those alone too. |
| Display HDR — `dd_hdr_option` | `auto` | The streamed display's HDR follows the stream. |
| Rotate the display — `dd_rotation_option` | `disabled` | The display keeps its orientation unless you ask it to follow the device. |
| HDR dynamic metadata — `hdr_dynamic_metadata` | `true` | HDR10+ and HDR Vivid let a capable display tone-map scene by scene; others ignore it. |
| HLG — `hdr_hlg` | `true` | Offered only to devices that ask for it. |
| Dolby Vision — `hdr_dolby_vision` | `true` | Offered only to devices that ask for it; others get the HDR10 or HLG stream underneath. |
| Windows HDR Calibration on virtual displays — `virtual_display_hdr_calibration` | `false` | That profile describes your monitor, not the device you stream to. |
| **Sound and input** | | |
| Stream audio — `stream_audio` | `true` | The host's sound goes to the device. |
| Audio to stream — `audio_capture_mode` | `system` | The selected playback device. [Game only](#game-only-audio) is a choice. |
| PCM and Dolby Digital audio — `audio_passthrough` | `true` | Offered only to devices that ask for it, for TVs and receivers that decode it themselves. |
| 7.1.4 surround — `audio_surround_714` | `true` | Offered only to devices that ask for it. |
| Use the device microphone — `stream_mic` | `true` | A device sends its microphone only when you turn it on there. |
| Controllers — `controller` | `true` | Devices can use game controllers on the host. |
| Haptics from game audio — `audio_haptics` | `false` | A matter of taste, so you turn it on. |
| Keyboard follows text fields — `keyboard_follows_focus` | `true` | The device's keyboard opens when a text field on the PC gets the focus. |
| Keyboard layout — `keyboard_layout` | `client` | Keys map with the layout the device reports. |
| Touch — `touch_mode` | `client` | Touches arrive as the device sends them. |
| Device-drawn cursor — `client_cursor` | `true` | A device that can draw the cursor itself moves it without waiting for video. |
| **Network, pairing and access** | | |
| Allow pairing — `enable_pairing` | `true` | New devices can pair. Turn it off once yours are paired. |
| Pair with a QR code — `qr_pairing` | `true` | Always on. Nightjar scans the code on Home or Devices instead of typing a PIN. |
| Local network encryption — `lan_encryption_mode` | `0` (Off) | Encryption costs performance on slower devices; the local network is yours. |
| Internet encryption — `wan_encryption_mode` | `1` (When the device supports it) | Streams leaving your network are encrypted whenever the device can; a device that cannot encrypt still streams, with a warning in the console. Tailscale counts as your network, and Tailscale encrypts it. |
| Client roaming — `client_roaming` | `true` | A phone moving from Wi-Fi to mobile data keeps its stream. See [Roaming](#roaming). |
| Console access — `origin_web_ui_allowed` | `lan` | The console and its API answer only on your local network. |
| **Permissions and sharing** | | |
| USB forwarding — `usb_forwarding` | `false` | A USB device on the PC is a real device; you choose to allow it, and each device needs its own permission. See [Clipboard, files and USB](#clipboard-files-and-usb). |
| File transfer — `file_transfer` | `false` | Off until you turn it on; each device also needs its own permission. |
| Shared folders — `shared_folders` | none | Nothing is shared until you add a folder and give it to a device. |
| Copy images — `clipboard_image` | `true` | Devices you allow to use the clipboard get images as well as text. |
| Host control, Sleep PC, USB devices, files and clipboard, per device | off | Granted per device under **Devices**; pairing never grants them. |
| Approve new devices, per device | first device with a certificate paired on, unless paired as **Watch only** | Pairs other devices from this one and nothing else. A browser can't use it, so one paired earlier doesn't count. Later devices get it only under **Devices**; an upgrade changes no device. |
| Can join co-op games, per device — `coop_join` | follows **Launch apps** | Joining a co-op game launches it, so a device that may launch apps may join until you take it away. |
| WebRTC server — `webrtc_server` | `false` | Only TVs and browsers that speak the Moonlight WebRTC protocol need it. |
| **Games, power and updates** | | |
| Close the app when the last device disconnects — `terminate_on_pause` | `false` | The game keeps running so you can reconnect. |
| Steam library — `steam_enabled` | `true` | Steam games appear with their covers. Every other launcher you install adds its games the same way. |
| Co-op players — `nucleus_coop_players` | `4` | The game's handler and the free controller slots can allow fewer. |
| When another device joins the running game — `guest_join` | `off` | Your own devices join or resume the running game as before; choose Ask the player to let guests in yourself. |
| What a guest may do — `guest_permission` | `controller` | A guest plays with its own controller; the keyboard and mouse stay yours until you hand them over. |
| Sleep after idle — `sleep_after_idle_minutes` | `0` (off) | The PC sleeps only when you ask it to. |
| Lock Windows when the last device leaves — `lock_on_last_disconnect` | `false` | Locking changes what you find at the PC after a stream, so it waits for you to ask, as in Parsec and Apollo. |
| Check for updates — `update_check_interval` | `86400` seconds | Once a day. |
| Update check time — `update_check_time` | empty | The interval decides when the host checks, not a time of day. |
| Install updates automatically — `auto_update` | `false` | You are told about an update and install it when you choose. |
| Include pre-releases — `notify_pre_releases` | `false` | Release candidates are offered only when you ask for them. |

These are defaults for **unset** keys, not a list of values you must copy into a configuration file.

Under **All settings → Picture**, the collapsed **Capture troubleshooting** group holds the capture options that matter only when a capture or encoder problem needs isolating: `capture_poll_interval_us`, `capture_predictive_poll`, `wgc_slot_aligned_publish`, `wgc_direct_encoder_input`, `gpu_compute_conversion` and `idd_push_fence_ring`. **Use the launch frame rate** (`limit_framerate`) and **Follow client size and frame rate changes** (`stream_reconfigure`), in the **Frame pacing** group, matter only to Artemis's Warp frame pacing and to Rubylight clients. `minimum_fps_target` is **Still-picture resend rate**: how often an unchanged picture is sent again, not a floor on a game's frame rate.

When several bitrate and pacing controllers are on, **Adapt bitrate on this PC** (`host_adaptive_bitrate`) wins over **Lower the bitrate when the link cannot carry it** (`link_rate_adaptation`) and **Adapt to network congestion** (`congestion_control`), which apply together (the lower bitrate wins), and a device that sets its own bitrate takes over from all of them. A manual **Pacing rate** (`pacing_max_bitrate_kbps`) turns off **Adaptive packet pacing** and **Slow down after video loss**; **Adapt bitrate on this PC** turns off both of those too, and **Adaptive packet pacing** turns off **Slow down after video loss**.

## Stream presets

`stream_preset` names a set of defaults for three AMD encoder settings and their NVIDIA NVENC counterparts: Lowest latency (`lowest_latency`), Balanced (`balanced`, the default) and Reference quality (`reference`). A preset only fills the covered settings that nobody set. A value in the settings file, in a device's overrides or in an app's overrides always wins, so you can pick a preset and still change one setting by hand. The preset's values are not written to the file; changing or clearing the preset changes the ones you did not set. Saving a covered setting in the console stores it, even if it equals the preset's value, so it stays when you pick another preset.

| Setting | Lowest latency | Balanced | Reference quality |
| --- | --- | --- | --- |
| `amd_max_frame_size` | `1` (one frame's share of the bitrate) | `auto` | `0` (driver default, no limit) |
| `amd_quality` | `speed` | `speed` | `speed` |
| `amd_preanalysis` | `false` | `false` | `false` |
| `nvenc_vbv_increase` | `0` (a one-frame VBV) | `0` | `400` (a five-frame VBV, the loosest) |
| `nvenc_preset` | `1` | `1` | `1` |
| `nvenc_temporal_aq` | `false` | `false` | `false` |

On NVIDIA each NVENC setting stands in for the AMD one above it: NVENC holds every frame, keyframes included, to its VBV and always has one, so Reference quality loosens it as far as NVENC allows instead of removing it. The measurements below were made with AMF; the NVENC values have not been measured on NVIDIA hardware.

- **Lowest latency** limits every frame, recovery keyframes included, to one frame's share of the bitrate. In [the October 7, 2026 recovery run](../rust/PERFORMANCE.md#october-7-2026-amf-rate-control-and-recovery-keyframes) a one-frame cap cut the largest 4K40 HEVC recovery frame by 66% with the same steady frame sizes and encode time. It names the cheapest quality preset and pre-analysis off as well, which are the defaults, so it changes nothing there. A value you set for either still applies and adds its measured cost (1.0 ms for HEVC and 12 ms for AV1 per 4K60 frame for the quality preset; 25 ms against 5-6 ms for pre-analysis).
- **Reference quality** removes the size limit on recovery keyframes (`amd_max_frame_size=0`, the driver default). No picture quality gain from that has been measured, so the preset claims none. What was measured is the cap itself: in the same recovery run a one-frame cap cut the largest 4K40 HEVC recovery frame by 66% and the encoded payload from 64 to 51 Mbps in the 63 Mbps run. Without the cap, recovery keyframes are larger on the wire, so use it on a fast, steady link. That run measured HEVC; H.264 with a one-frame cap was not measured separately (H.264 and AV1 were only checked at a two-frame cap, for compatibility). Pre-analysis, the quality presets and the quality rate-control modes stay off: the measurements show no consistent picture gain for their encode time. Packed 4:4:4 is not part of any preset: on a Radeon 780M (8-bit HEVC) it improved still text and UI above about 5 Mbit/s of actual bitrate, but scrolling at 5 Mbit/s was 0.8 dB worse and the first frame was worse below 20 Mbit/s; it added 1.2 to 2.1 ms of encode time and 0.7 to 1.2 ms of hardware decode, and was worse at 2 Mbit/s, so it stays the separate **Full chroma** choice ([measurements](../rust/PERFORMANCE.md#october-23-420-against-packed-444-on-text-and-ui-and-afmf-capture-b3-b7-measured-on-the-780m)). On an RX 7900 XT with six text and UI scenes the picture gain held (3 to 17 dB on still text from a 5 Mbit/s target), and so did the encode cost: 1.2 ms more at 1080p and 2.0 ms at 1440p ([measurements](../rust/PERFORMANCE.md#november-1-packed-444-as-the-reference-default-rx-7900-xt)).
- **Why no preset sets `amd_rc`.** Setting it makes the encoder require that mode and fail loudly if it is unavailable. Constant bitrate is not a quality setting either: in the [AMF quality sweep](../rust/PERFORMANCE_WORK.md#amf-settings-per-codec-quality-at-a-given-bitrate) it scored slightly higher on the first clip only because it spends more of the budget, the 99th-percentile frame grew by 30-67%, and the 48 HDR encodes found no reason to change the rate control. Rate control stays at the default in every preset.

The console shows the selector as **Stream preset** under **Settings → Picture** and in the **Preset** group of **All settings → Picture**, and, below it, which covered settings have a value of their own ("Custom"), including one equal to the preset's. Settings the preset covers show its values as their defaults, and Reset returns to them. A name the host does not know still loads, and the host uses Balanced; the console shows the stored name with an error instead of showing Balanced.

A device or an app can name its own preset with `stream_preset` in its overrides (`config_overrides` for a device, `config-overrides` for an app). The most specific name wins (an app's settings for the device, then app, then device, then host; see [one game on one device](#one-game-on-one-device)), and it fills only the three settings that no layer set: an explicit value of the host, the device or the app is never replaced by a preset. The host refuses to save a device or app preset name it does not know, or one that is not text. The device editor has a **Stream preset** control for the device's own name; the app editor sets it through the app's overrides, and for one device in its **Per device** panel.

## Capture and video

### Graphics card and capture method

**Graphics card** (`adapter_name`) lists all hardware GPUs reported by Windows DXGI, including GPUs with no attached monitor on hybrid systems. **Automatic** uses the GPU that owns the captured display. A GPU appearing in the list does not confirm encoder or codec support; that still depends on its hardware and driver. With a GPU selected, virtual displays are created to render on that GPU. WGC can capture a display that another GPU renders; Desktop Duplication cannot, so when it is the capture path in use and the display renders elsewhere (a physical monitor on the integrated GPU, or a driver that ignored the request), the stream captures and encodes on the display's GPU and the stream card says so.

**Keep Capture method on Automatic and compute conversion enabled** for normal use. On Radeon and NVIDIA graphics cards whose driver supports it, the compute path lets capture and colour conversion run alongside graphics work. Unsupported textures can fall back to the graphics path. The internal `wgc_compute_copy` switch also defaults to `true`; it is primarily useful for controlled troubleshooting.

**Frames from Rubylight's display driver.** On a virtual display Rubylight created, Automatic captures from the driver itself: it copies each frame Windows composes into textures it shares with the host, so neither WGC nor Desktop Duplication runs, and the compute conversion reads those textures in place (`idd_push_zero_copy`, default `true`). To select it explicitly, set **All settings → Picture → Capture method** to **Direct driver push** (`capture = idd-push`). It applies to Rubylight's own virtual displays and falls back to WGC when unavailable, including on a physical monitor. Choosing WGC or Desktop Duplication explicitly requests those methods. The stream captures with WGC instead, and the stream card says why, when the driver is not installed, when another stream with different capture settings already captures that display, or when the display's frame push failed twice before delivering a picture. A display that failed is tried again once it is recreated or renders on another GPU. Save and end all streams using the display before starting a new session to compare capture methods; simultaneous streams may share an already-open capture.

**GPU fence transport** (`idd_push_fence_ring`, default `true`) is the transport for direct driver push with compute conversion and in-place capture enabled. It uses D3D12-owned shared textures and GPU fences while the display driver continues using D3D11. Set it to `false` to use the keyed-mutex ring instead. It requires a compatible Rubylight display driver; missing support, failed setup or failed synchronization uses the keyed-mutex ring and shows a warning. A rejected fence transport stays rejected through capture restarts on that display. Recreate the virtual display or restart the host to retry. Save and end all streams using the display before changing it. WGC and Desktop Duplication remain available. This option does not require the new native D3D12 IddCx swap-chain API.

**Downscaling uses an antialias filter automatically.** Supported GPU conversion paths average each output pixel's covered source pixels for reductions up to 8× per axis. This helps when the desktop and stream sizes differ, including fractional reductions. Native resolution, enlargement and aligned 2× reduction keep the existing filter. Larger reductions retain the previous filter to bound GPU work. Existing HDR transfer handling, cursor composition and aspect-ratio handling remain in the same conversion pass.

**AMD Direct Capture** (`capture = amd`) reads the picture the Radeon driver presents on a display connected to a Radeon graphics card, through AMF's display capture. It waits for each present instead of sampling the screen, up to the display's refresh rate (at most 200 frames per second, AMF's limit). Frame generation's extra frames, such as AMD Fluid Motion Frames, are not streamed: they reach no capture method, this one included. It needs a Radeon driver with AMF 1.4.30 or newer and captures the desktop pointer into the picture. On another GPU, an older driver or any setup failure, capture falls back to Windows Graphics Capture and the console shows a `capture_backend` warning. Automatic never selects it; choose it under **All settings → Picture → Capture method**. The log says `AMD Direct Capture started` with the display and rate, and `AMD Direct Capture stopped` with the presents seen and missed.

The installed service runs WGC through a helper in the signed-in user's session. WGC can fall back to DDX when unavailable. Selecting a particular capture method is useful when investigating a problem; check **Logs** to see which path actually opened.

### The app's window in place of a display

**What to stream** (`capture_window`, default `display`) set to `window` streams only the launched app's window. Set it for one app on the app's **Display** panel in **Games**, for one app on one device in its **Per device** panel, or for every app under **All settings → Picture → Capture**. A device can also set it in its own `config_overrides`.

- **Which window.** The largest shown window of the programs the app started, and of everything those started (the same process tree game-only audio follows). Dialogs, tool windows and windows smaller than 64 × 64 pixels are skipped, and the window being streamed stays while it qualifies. **Window's program** (`capture_window_exe`, such as `game.exe`, in any case, with or without `.exe`) takes only that program's windows, and also finds it when a launcher handed the game to a process it did not start; a copy the app started wins over one started at the PC. **Window title contains** (`capture_window_title`) takes only windows whose title contains the text, in any case. Both are empty by default.
- **Capture.** Windows Graphics Capture captures the window, on the graphics card and in the colour space (SDR or HDR) of the display that shows most of it. A capture method set to Desktop Duplication or direct driver push captures whole displays only, so such a stream uses WGC and the stream card says so; Automatic needs no change. The mouse pointer is drawn into the picture while it is over the window. Windows 10 version 1903 or later is needed.
- **Picture.** The window keeps its shape: the GPU conversion scales it to the stream's size, with black bars where the shapes differ. Resizing, a DPI change, switching between windowed, borderless and fullscreen, and moving the window to another monitor (and with it to that monitor's HDR or SDR) reopen the capture at the window's new size or on its new display, which takes a moment.
- **Input.** Absolute mouse, touch and pen input map onto the window: a point on the stream, black bars excluded, is the same point of the window's visible frame on the PC's desktop, wherever the window is; a point on a bar lands on the window's edge. The window's position is read again every 100 ms. Relative mouse, keyboard and controllers work as on a display.
- **When there is no window.** Until the window appears, while it is minimised, and for 5 seconds after it closed (a splash screen giving way to the game), the stream shows the stream's display and the device shows why. **When the window closes** (`capture_window_closed`) then either keeps showing the display (`display`, the default) or ends the stream with the reason (`end`). A window that comes back is streamed again. When the streamed window's process exits, though, the stream ends whatever `capture_window_closed` says.
- **Apps without a program of their own.** The desktop and detached commands start nothing the host can follow: name the program, or the stream shows the display and says so. Steam and Playnite games use the game the host tracks.
- **Co-op players** stream their own game window already and ignore these settings.

### Codecs, full chroma and HDR

H.264, HEVC and AV1 depend on the encoder and client. For `hevc_mode` and `av1_mode`, the choices are `0` Automatic, `1` Off, `2` SDR only and `3` SDR and HDR. Restart after changing advertised codec support. AMD's encoder makes 4:2:0 only, so stock Moonlight gets 4:4:4 from an AMD host through PyroWave alone. On an NVIDIA GPU the host also streams native 4:4:4 in H.264 (8-bit) and HEVC (8-bit and 10-bit) to a device with its YUV 4:4:4 option on, where the GPU's encoder reports 4:4:4 support. The startup check encodes each mode and advertises the ones that work, and the console's host readiness lists them; there is no host setting. AV1 4:4:4 is not available on NVIDIA. A device that asks for a 4:4:4 mode the host did not advertise gets an error naming the reason, never a 4:2:0 stream instead. For full chroma over HEVC or AV1, keep **Full chroma** (`experimental_packed_444`, on by default; it only offers the modes, and a stream uses one only when its client asks) on under **All settings → Picture → Codecs**, and set Nightjar's **Sharper colored text** to **Automatic**. The host then packs each 4:4:4 frame into an ordinary 4:2:0 picture and Nightjar puts it back together; stock Moonlight never sees the packed layout.

For **native HDR**, enable HDR in the client and use an HDR-capable display and encoder path. Leave **Display HDR** on Automatic so Rubylight can set the source display appropriately. **10-bit SDR instead of HDR** (`prefer_sdr_10bit`, default `false`) deliberately keeps the stream in SDR; leave it off when you want HDR. A client can also ask for 10-bit SDR itself: when it negotiates a 10-bit format (`dynamicRangeMode=1`) and sends `hdrMode=0` (on /launch or /resume), Rubylight streams 10-bit SDR and leaves the display in SDR, whatever this option says. Without `hdrMode`, or with `hdrMode=1`, a 10-bit format stays HDR unless `prefer_sdr_10bit` is on.

**HDR dynamic metadata** (`hdr_dynamic_metadata`, default `true`) adds per-frame brightness statistics to HDR streams: HDR10+ (SMPTE ST 2094-40) in HEVC and AV1, and on HEVC also HDR Vivid (CUVA 005.1). Displays that tone-map with HDR10+ can then follow each scene; clients without it ignore the metadata. HDR10+ is written as profile A, without a target display, and the first frames, before any measurement, carry values from the stream's static HDR10 levels. They are measured on the GPU from the captured picture while the frame encodes, behind the compute conversion (`gpu_compute_conversion`) and the D3D11 conversion alike; PyroWave carries none. Packed 4:4:4 streams carry static HDR10 only, from the control channel: the device unpacks the picture in its own shader, so no decoder would read the statistics, and the host logs that and runs no statistics pass. If the statistics pass does not start or fails during a stream, the stream goes on with static HDR10 only and the web UI says why. Static HDR10 metadata is the same either way. One switch covers both formats: turn it off if a display handles brightness worse with it.

**HLG** (`hdr_hlg`, default `true`) offers HLG (ARIB STD-B67) on HEVC, AV1 and PyroWave HDR streams to devices that ask for it (`HdrFormats` in serverinfo, `x-rl-video.hdrFormats` in DESCRIBE, only while an HEVC, AV1 or PyroWave Main10 encoder is there; PyroWave adds the offer flag 128, since older hosts refuse HLG on PyroWave). The host then encodes the captured picture with the BT.2100 HLG curve, graded for a fixed 1000-nit display, and signals transfer 18 in the bitstream; the device's screen tone-maps from there, and light above 1000 nits keeps the little headroom the signal has before it clips. Devices that do not ask, stock Moonlight among them, get HDR10 as before. HEVC HLG streams carry HDR Vivid statistics while **HDR dynamic metadata** is on, capped at 1000 nits like the picture; HDR10+ describes HDR10 streams only, and AV1 has no HDR Vivid. Packed 4:4:4 streams stay HDR10, and the host logs why.

**Dolby Vision** (`hdr_dolby_vision`, default `true`) offers Dolby Vision profile 8.1 on HEVC HDR streams and profile 10.1 on AV1 HDR streams to devices that ask for it (`HdrFormats` in serverinfo, `x-rl-video.hdrFormats` in DESCRIBE, only while an HEVC or AV1 Main10 encoder is there), and profile 8.4 on HEVC while **HLG** is on. Each frame then carries a Dolby Vision RPU built from the same per-frame statistics as HDR10+, over the stream's HDR10 base layer (HLG for 8.4): on HEVC in a NAL unit at the end of the frame, on AV1 in a metadata OBU before it. Devices without Dolby Vision and stock Moonlight see the HDR10 or HLG stream underneath. Until the first sample, or if the statistics stop, the RPUs carry the stream's static light levels and the web UI says so. Packed 4:4:4 streams carry static HDR10 instead: the device unpacks the packed picture itself, so a Dolby Vision decoder would tone-map the packed layout; the host logs this and the web UI shows the reason.

### PyroWave

**PyroWave needs a compatible client** and much more bandwidth than conventional codecs. Desktop and game test scenes on AMD give two warning levels. The floor warns about severe detail loss; the recommendation targets clean pictures in those tests. Passing the floor alone does not mean a clean picture.

| Stream | Severe-loss floor | Recommended |
| --- | ---: | ---: |
| 720p60 | 139 Mbps | 277 Mbps |
| 1080p60 | 187 Mbps | 399 Mbps |
| 4K60 | 747 Mbps | 1593 Mbps |

These values are rounded up to whole Mbps. Below 1080p the floor/recommendation are 2.5/5 bits per pixel per frame; at 1080p and above they are 1.5/3.2. The smaller desktop's one-pixel text needed more bits per pixel. Rates scale with pixel count and frame rate within each band: halve them at 30 fps, double them at 120 fps. Other sizes are estimates; actual quality depends on text size, textures and the picture. The console stream card uses red below the floor and amber below the recommendation, and the log distinguishes both. The [October 12, 2026 Rubylight 2.2.9 validation](../rust/PERFORMANCE.md#pyrowave-decoded-pictures-and-quality-budgets) rechecked these budgets on synthetic desktop and game pictures; it measures decoded quality, not sustained network throughput. [Original October 7 method and criteria](../rust/PERFORMANCE.md#october-7-pyrowave-bitrate-from-representative-pictures).

The PyroWave bitrate you set in the client is the stream's total on the wire, as in Vibeshine. The host keeps audio with its parity and 500 kbps for control. With record framing (Nonary's Moonlight, Nightjar, Aurora), each picture then gets what fits after packet overhead and one block of critical FEC, so a rate the client calibrated is never exceeded. The picture therefore gets about 15–20% less than the number you set; set the bitrate to what your network carries, not to the picture rate you want. The 4K60 recommendation needs more than gigabit Ethernet. Client and host limits still apply: stream setup allows up to 2 Gbps, while the client's runtime `/bitrate` endpoint caps changes at 500 Mbps. If the client or network cannot carry the recommended rate, use HEVC or AV1. Enabling PyroWave does not force ordinary Moonlight clients to use it.

### Size and frame rate changes, and VRR

**A Rubylight client can change the stream's resolution and frame rate without reconnecting**, for example when a foldable phone opens its inner screen. The encoder restarts at the new size with a keyframe and pacing follows the new rate; the PC's display keeps its mode and the picture is scaled to fit. `stream_reconfigure` (default `true`) turns this off for the host, a device or an app; clients that never ask are unaffected. [Control messages](control-messages.md) describes the request.

**VRR is client-negotiated.** A VRR request can use a 1000 Hz virtual display when automatic virtual refresh is enabled. That is the host's virtual display rate, not a claim that your TV or monitor refreshes at 1000 Hz. Client and display support still matter.

### Packet pacing

**Packet pacing limits video bursts.** The default remains the existing route-based policy: with `pacing_max_bitrate_kbps = 0`, H.264, HEVC and AV1 use up to 800 Mbps on wired or unknown routes. Confirmed host Wi-Fi uses twice the encoder bitrate, with at least 25% headroom above packetized demand. A known physical Ethernet link limits pacing to 80% of its reported speed. Hyper-V vSwitches use their physical adapter when Windows exposes an unambiguous binding. The host's link speed does not establish the capacity of the client or the rest of the path.

**Repeated video loss slows packet delivery** (`keyframe_loss_pacing = true`). The guard activates when the client reports a keyframe it could not repair, asks again for the keyframe it was just sent, or reports three distinct unrecoverable frames within two seconds. A client that announces FEC status (`x-ml-general.featureFlags` 0x01: Moonlight, Artemis, Nightjar) reports every frame it lost, so its repeated keyframe request is its decoder asking again, not loss: it is logged as "video loss evidence not counted" and does not activate the guard. Frames the host could not send whole because the network send buffer was full are loss too: three of them within two seconds, or one keyframe, activate it, named "lost on send" in the log, and the client's report of the same frame is not counted a second time. When only the host saw that loss (the host's network adapter stalled, the client reported nothing), the slower pace lifts after 10 clean seconds and 120 frames like a lost first keyframe; more of it while the pace is slow restarts the 10 seconds. In a stream that already slowed down once (an earlier lift, or a route remembered from a previous session) it holds for the rest of the stream at once. Loss only the host saw is never saved, and a repeated request for a keyframe the host's socket refused is the host's loss too. Reports for several blocks of the same frame count once. Nothing the host did itself counts: a keyframe the host sent on its own (Alt+Tab, a bitrate change, a restarted capture or encoder) is not a keyframe the client "asked again" for, even if the client asked just before it, and requests and ordinary-frame reports within one second after it are set aside. The same goes for the first half second after the stream's first keyframe (a client that has not shown a picture yet asks again within about 100 ms). Keyframes sent to recover from loss open no such window. The log says what set it off (the frame numbers) and what was set aside. Stale ordinary-frame reports, isolated losses and fully repaired frames do not trigger it. The stream is then paced like a confirmed Wi-Fi route for the rest of the session: twice the encoder bitrate, at least 1.25 times wire demand, within the route ceiling. The encoder bitrate does not change. With the default sender thread enabled, loss feedback received before sender selection moves the next output to that thread, avoiding a slow inline send on capture.

Sustained loss the client reported is saved with the paired device's authenticated identity, its IP address, the host network interface the route leaves by, and the client's own link speed when it reports one (Nightjar does on wired links), at stream end. Loss that was only the stream's first keyframe, with nothing else seen, holds the slower pace for that stream until it has been clean for 10 seconds, and is not saved; a second latch in the same stream, or more loss the client reports while it holds, is sustained (loss only the host saw while it holds restarts the 10 seconds). The next eligible session at that address starts at the slower pace, including its first recovery frame; the one after tries the ordinary route pace again. A different host interface (another network adapter; the adapter is told apart, not the network it is joined to) or a different client link starts without history, and records saved by earlier releases are ignored. A completed probe with at least five seconds and 120 sent frames and no new loss clears that route's history. Entries expire 24 hours after observed loss; simply reusing remembered pacing does not extend that lifetime. **Devices → Reset network history** clears the device's stored routes for subsequent streams, without changing a running stream. A different address starts independently. A client that reports its wired link (`x-ss-video[0].pyrowaveLinkMbps`) caps conventional-codec pacing at 4/5 of the slower of the two network ports from the first packet: a 1 GbE client stays at 800 Mbps, a 100 Mbps client gets 80. A hint below the encoder bitrate does not pace the stream under it: the pace is then 1.1 times the bitrate, and the host log carries one warning naming both values. Only the stream's first keyframe, and a keyframe the client asks for within a second of it, count as the start; a keyframe lost later, such as one from an encoder rebuild, is not provisional. The 10 clean seconds are counted from the last loss. Manual pacing, either adaptive controller, disabled loss pacing, and PyroWave do not use this memory.

Adaptive error correction keeps the configured protection for 30 seconds after loss, then decays by one percentage point every five seconds of clean feedback. At the default 20% ceiling it reaches the 5% floor after 100 clean seconds. Recovery frames retain the ceiling, and clients without FEC feedback retain the configured percentage.

The original keyframe guard was measured through a 100 Mbps loopback bottleneck with a 16 or 64 KiB queue: 1440p60 AV1 and HEVC streams at 40 and 60 Mbps went from no frame received whole in 20 seconds to all but the first few. Those historical measurements do not measure the newer repeated-frame trigger or remembered routes. [Measurements](../rust/PERFORMANCE.md#october-14-keyframe-loss-on-a-slow-port).

**Adaptive packet pacing is off by default** (`host_adaptive_pacing = false`). It is an option for connections with burst-related loss when manual pacing is inconvenient. Enabling it starts each conventional stream at twice the encoder bitrate or 1.25 times wire demand, whichever is higher, within the route ceiling. A suitable fixed limit is simpler when the constrained path is known.

**Wi-Fi clients get it automatically** (`wireless_adaptive_pacing = true`). The host sees only its own link, so a wired host cannot tell that the client sits behind an access point. A client that reports a Wi-Fi or cellular route to the host with the optional SDP attribute `x-rl-video[0].clientWireless:1` (Nightjar does; absent or `0` means unknown and changes nothing) gets the adaptive controller below on H.264, HEVC and AV1, even though `host_adaptive_pacing` is off. The controller starts at twice the encoder bitrate (at least 1.25 times wire demand), so the first keyframe is not sent as a line-rate burst. It then probes up in steps of at most 12.5% while the link stays clean, up to the host route's ceiling, and backs off on loss, exactly as for `host_adaptive_pacing`. Keyframe-loss slowdown and its remembered routes are off for such a stream, because the controller owns pacing, and the encoder's keyframe size cap follows the controller's rate as it does for `host_adaptive_pacing`. Where the controller does not own an H.264, HEVC or AV1 stream (`wireless_adaptive_pacing = false` or host adaptive bitrate), the hint instead applies the Wi-Fi route policy from the first frame (twice the encoder bitrate, at least 1.25 times wire demand, no probing), which also gives the smaller keyframe cap of a Wi-Fi host. A manual pacing limit always wins; the hint changes nothing there. For PyroWave the hint only stops the client's wired-link hint from setting the starting rate, so it starts from the demand-based rate instead, and a pace the client calibrated still wins.

During startup, busy video, recent standard client pings and stable nonzero RTT allow tentative increases of up to 12.5%, separated by at least half a second and a feedback-delay guard. A new loss report during a probe restores the preceding rate and switches to slower recovery. There is no timer-only jump to the maximum. Each reconnect starts conservatively again: a client can move between Ethernet and Wi-Fi without changing its identity or the host's route. Startup pacing adds transmission time, especially to a large first keyframe; with a 40 Mbps encoder and continuously healthy traffic, reaching an 800 Mbps ceiling takes about ten seconds.

Outside tentative probes, the optional controller reduces bursts after repeated, validated client FEC reports show substantial loss. It changes packet spacing without lowering encoder bitrate or FPS. A half-second sample needs two distinct failed frames, or multiple recovered frames with at least 2% of sent packets missing. Old and duplicate reports cannot repeatedly penalize a replacement rate. The rate stays at least 25% above current packetized video demand, including FEC, minimum parity, encryption and network headers, unless the route ceiling is already lower. This floor prevents pacing itself from creating a persistent backlog; it cannot make an overloaded path carry the stream.

Recovery needs recent standard client pings, active video traffic, stable RTT and an initial eight-second clean hold. Small rate increases are tentative; a failed probe restores the previous rate and extends the next recovery hold, up to a minute. Paused video, absent standard pings or missing RTT does not authorize a faster rate. Stock Moonlight does not acknowledge every successfully received video frame: control pings establish activity, not video-path capacity. Loss during probing remains possible, and clients without loss reports cannot trigger loss-based rollback. The session API exposes `packet_pacing`, including startup state, and rate changes appear in the host log. The separate **Host adaptive bitrate** option keeps its existing opt-in quality/bandwidth controller and disables this additional burst controller.

Conventional video batches contain at most about one millisecond of traffic at the selected rate, or one datagram if it takes longer. `video_max_batch_size_kb` remains an additional payload limit (16, 32 or 64 KiB; default 64). With `video_send_thread` enabled (the default), adaptive packet pacing reserves the dedicated video sender from the start so pacing waits do not block capture. Large keyframes can still take multiple frame periods. Startup and steady-state timing are reported separately in the measurement record.

`video_send_mmcss_only` (default `false`) runs the video packetizer and paced sender with the multimedia scheduler's boost alone, as the input thread does, without the thread priority that otherwise cancels it. It is for measuring under CPU load and is off by default.

`video_send_early_blocks` (default `true`) hands a frame to an idle paced sender one FEC block at a time, each as soon as its parity and encryption are done, so the first packets leave while the rest of the frame is still being built. This applies to PyroWave and to H.264, HEVC and AV1 frames on the dedicated sender; a busy sender still gets whole frames, which a newer PyroWave frame or a keyframe can replace. At 4K60 with frames of about 790 KB the first packet left about 0.55 ms sooner on HEVC and AV1; a frame that fits one block gains nothing ([November 8](../rust/PERFORMANCE.md#november-8-sending-fec-and-amf-output-after-390)). Set it to `false` to build every block before the first leaves.

`pacing_follow_client_link` (default `false`) lifts the 800 Mbps ceiling of H.264, HEVC and AV1 on a wired route when the device reports its own link (`pyrowaveLinkMbps`): the ceiling becomes 80% of the slower of the host's and the device's link, and never less than 800 Mbps. It is off by default.

`fec_gfni` (default `true`) computes error-correction parity with the GFNI instructions on processors that have them (AMD Zen 4 and newer, Intel Ice Lake and newer); the bytes are the same as with the AVX2 path, which `false` forces. On a Ryzen 7 8845HS packetizing with error correction was 2 to 4 % faster on large frames ([November 8](../rust/PERFORMANCE.md#november-8-sending-fec-and-amf-output-after-390)).

`video_slice_delivery` (`auto`, `off`, `2` or `4`; default `auto`) sends an H.264 or HEVC P-frame in that many FEC blocks, each as soon as the encoder's slices for it have left AMF, instead of after the whole frame. `auto` uses 4 blocks when the stream's picture has at least three quarters of the pixels of 4K (3840x2160, 4096x2160 or 5120x1440, not 3440x1440 or smaller) and the stream does not use the intra refresh its client asked for; otherwise it is off. A mid-stream size change decides again when the encoder is rebuilt, and the send path follows it. The gain is the time between the encoder's first slice and the whole frame, which grows with the picture: a 4K60 picture arrived about 1.4 ms sooner, a 1080p one no measurably sooner. It is set in the configuration file only, not in the console. Keyframes, recovery frames and AV1 always go out whole, and a stock Moonlight or Nightjar client reassembles the frames unchanged. Turning it on asks AMF for slice output with at least one slice per block and turns intra refresh off. A stream with its own send thread (current or adaptive pacing) streams the same way: the packetizer thread builds each block as the units arrive and the paced sender sends it. If a frame ever passes the byte cap its blocks were planned for, the host logs a warning, counts it in `slice_delivery_overflows`, requests a recovery frame and sends whole frames for the rest of the stream. `BUTTERPOLLO_TEST_SEND_LOSS=percent:seed[:air]` drops video datagrams at random for tests; unset it otherwise.

Set `pacing_max_bitrate_kbps` to a positive value in **kbps** to override automatic burst control, including for PyroWave. For example, `120000` paces at 120 Mbps. Conventional video retains its floor of 110% of the stream bitrate and ceiling of 80% of a known Ethernet link. PyroWave treats the setting, the client's calibrated pace and 95% of either endpoint's reported link as hard ceilings: stream demand cannot raise them. The host warns if the wire budget exceeds the resulting rate. Reconnect after saving. [Measurements and pacing math](../rust/PERFORMANCE.md#october-14-conventional-video-pacing).

PyroWave starts unmeasured and wireless paths at 1.25 times their estimated wire demand (at least 110% of the stream bitrate before that headroom), within the ceilings above. A wired host alone does not select line-rate bursts. A client reporting its wired link can retain fast wired pacing. Opt-in adaptive packet pacing also applies to PyroWave; the loss guard and Wi-Fi adaptive pacing do not. Loss that error correction cannot repair lowers PyroWave's bitrate as for every codec, but not below its severe-loss floor (see [Lower bitrate when the link cannot carry it](#lower-bitrate-when-the-link-cannot-carry-it)). The pace comes down with it only where it follows the stream's demand: a client pace hint, `pacing_max_bitrate_kbps` or a client's reported wired link keeps setting it. Calibration and streaming use the same segmented sends, retry handling and batches of at most one millisecond of wire traffic, 64 packets and 65507 payload bytes. Error correction can adapt up to `pyrowave_critical_fec_percentage`; clients without reports keep that configured protection. Record-capable clients protect the coarse picture. Legacy clients use standard whole-frame parity within their four-block limit; large frames may receive less protection and show a warning. Their parity uses some of the existing picture budget; set PyroWave error correction to 0 to retain the unprotected format. Every encoded frame is limited to one nominal frame budget, including the first frame after a pause.

### AMD AMF encoder settings

**Leave AMF's low-latency mode and AV1 latency mode on Driver default.** In the [October 12, 2026 Rubylight 2.2.9 property sweep](../rust/PERFORMANCE.md#retained-amf-property-sweeps-october-12), default ultra-low-latency usage already read back internal low latency for H.264/HEVC and AV1 latency mode 3 on the RX 7900 XT. Explicitly forcing the same settings produced similar timings across three runs, not exact equality or a demonstrated speedup. With Low latency or Transcoding usage, single auto/forced pairs changed the readbacks from off to on and showed mean reductions of 0.30-0.31 ms for HEVC and 1.01-1.37 ms for AV1. These are eight-second synthetic 1440p120/20 Mbps component tests, not full-stream latency measurements. The [October 7 audit](../rust/PERFORMANCE.md#october-7-amf-low-latency-mode-and-av1-latency-mode) documents earlier measurements and the RX 9000 HEVC-freeze concern; forcing these settings remains opt-in.

**Adaptive quantization (`amd_vbaq`) is off for H.264 and on for HEVC and AV1 when unset.** In the [October 12, 2026 Rubylight 2.2.9 settings comparison](../rust/PERFORMANCE_WORK.md#current-amf-quality-settings-comparison), explicitly turning H.264 AQ on reduced VMAF in all four cases: by 1.44 and 11.89 points on the two clips at 1440p120/20 Mbps, and by 0.06 and 0.41 at 50 Mbps. Turning AQ off gave mixed HEVC results, while AV1 VMAF rose in all 14 tested cases by 0.0009-0.2301 points in SDR and 0.0051-0.4822 in converted HDR. Those small AV1 gains do not establish a universal visual-quality benefit; the HDR scores also use converted SDR footage. Set AQ to On or Off to compare your own content, or Driver default to leave it to the driver. The [October 8 findings](../rust/PERFORMANCE_WORK.md#amf-settings-per-codec-quality-at-a-given-bitrate) remain a separate historical result.

The [current default-quality results](../rust/PERFORMANCE_WORK.md#current-amf-default-quality) cover 32 cases; the [settings comparison](../rust/PERFORMANCE_WORK.md#current-amf-quality-settings-comparison) adds 116 alternatives for quality preset, rate control, AQ, usage and high-motion boost. Each case encoded and scored exactly 180 frames, with no minimum VMAF pass threshold. CBR increased p99 frame size in every tested case; its score changes depended on codec and content. Ten high-motion-boost requests completed but lacked effective-setting readback, so their application is unverified. Encode-time deltas use earlier baselines rather than randomized matched trials. HDR rows convert SDR footage with 203-nit white and do not establish native-HDR perceptual quality.

**Leave split-frame encoding on Automatic.** AMF can split one HEVC or AV1 frame across a Radeon's two encoder engines (`amd_split_frame`), and the driver decides whether it does. Automatic asks for it only when the GPU has two engines and the driver has it off; On asks whenever there are two engines, and Off disables the property. In the [October 12, 2026 Rubylight 2.2.9 sweep](../rust/PERFORMANCE.md#retained-amf-property-sweeps-october-12), RX 7900 XT readbacks were 1 for Automatic/On and 0 for Off. Identical-input synthetic throughput was similar: for example, two-run means for 4K HEVC were 197.32 / 197.37 / 197.36 FPS respectively. This does not mean every frame had identical timing or prove how the driver used its engines. The sweep completed 708/720 calls; twelve pre-analysis cases failed and remain documented. It did not assess decoded quality. H.264 has no split-frame property; single-engine GPUs get no property write. [Historical October 7 measurements](../rust/PERFORMANCE.md#october-7-amf-split-frame-encoding) remain separate.

**Hardware color conversion** (`amd_vpe`, default off) converts and scales captured frames with AMF's video converter instead of Rubylight's shaders, on the Video Processing Engine where the graphics card has one and on the driver's compute fallback where it does not. It runs on the encoder's graphics device and hands its output to the encoder without a copy, so the stream does not use the compute queue while it is on. It covers SDR, HDR10 and HLG streams with YUV 4:2:0; frames with the desktop pointer or couch menu drawn by Rubylight, HDR brightness scaling, full-chroma (4:4:4) streams and NVIDIA NVENC keep the shaders, frame by frame. A failure to start or convert returns the stream to the shaders with an `encoder_vpe` warning. Every 10 seconds the log line `vpe_conversion` reports how many frames each converter took, their conversion times and the capture-to-submit time; the stream timing line's `capture_age_at_submit_ms` gives the same measure with the setting on or off, for comparing it with the shaders. The default stays off until the offline comparison and live timings in the [hardware matrix](hardware-matrix.md) show equal or better quality and equal or lower latency on a card.

**AMF automatically sizes recovery frames to packet pacing.** In the **AMD AMF** group of **All settings → Picture**, the controls below use the stream's encoder bitrate and frame rate. Peak bitrate and VBV remain at driver defaults; maximum frame size defaults to `auto` when unset. Existing explicit settings, including `0` and the previous `2` default when saved explicitly, are preserved. Reconnect after saving. Initial values appear in the `AMF encoder settings` log line; each recovery picture reports its submitted cap after runtime adjustments.

| Key | Values | Meaning |
| --- | --- | --- |
| `amd_peak_bitrate_ratio` | `0`, or `1`–`2` | Peak bitrate as a multiple of the stream bitrate; the console offers 1×, 1.5× and 2×. This is not an individual-frame cap. |
| `amd_vbv_buffer_frames` | `0`, or `0.5`–`2` | Rate-control buffer in nominal frame budgets. One budget is bitrate divided by frame rate, including fractional rates. |
| `amd_max_frame_size` | `auto`, `0`, or `1`–`8`; default `auto` | Automatic budgets about two frame periods at the live packet pace, allowing 2–8 nominal frames after FEC/header allowance. It also stays within the largest frame that keeps its full FEC percentage in Moonlight's four FEC blocks (about 1.17 MB at 20% with 1392-byte packets; under `rlFecPercentage=auto` the percentage chosen for that frame; without FEC the four-block packet limit, about 1.40 MB), unless 2 nominal frames are already larger. AMF does not hold an inter frame to the cap, so with `auto` an inter frame past that full-FEC limit (a scene cut) is encoded once more as a keyframe, which is sent instead (only where the cap keeps that limit, and not with `video_slice_delivery`). PyroWave needs no cap: its rate control gives every picture, a scene cut included, at most one frame's budget. Explicit numeric values retain their fixed nominal-frame budgets; `0` leaves the property to the driver. Uses `MaxAUSize` for H.264, `HevcMaxAUSize` for HEVC and `Av1MaxCompressedFrameSize` for AV1. |

Automatic frame caps are applied before the first encoded picture and reconsidered as pace, bitrate or frame period changes. Recovery FEC and conservative packet/encryption overhead are included. Small increases of at most 10% avoid repeated driver writes; decreases apply immediately. The driver can reject or exceed this soft limit, and the two-frame minimum can exceed the intended transmission window on an undersized link. Existing profiles that explicitly saved `2` retain it; choose **Automatic** to use the new policy.

Each recovery picture logs `video recovery frame` with its encoded size, the driver-read cap associated with its submission, actual packetized wire budget, pace, estimated wire time and pacing reason. `sender_work_ms` measures work in the selected sender (including interleaved packetization in inline mode), not receiver decoding or display latency. The counters can be compared with the stream's `fec_unrecoverable_frames` diagnostics. Missing cap readback is left unknown rather than reported as an applied limit.

These are encoder bit budgets, not extra queued frames or packet-pacing settings. A smaller budget trades picture quality for smaller frames. Intra refresh remains client-negotiated for H.264 and HEVC and does not replace an explicit recovery-keyframe request. AV1 never uses it: a lost AV1 frame is recovered with an invalidated reference frame, or a keyframe when `amd_ltr_frames` is `0` ([measurement](../rust/PERFORMANCE.md#october-27-av1-intra-refresh-and-recovery-after-a-lost-frame)). See the [rate-control measurements](../rust/PERFORMANCE.md#october-7-2026-amf-rate-control-and-recovery-keyframes) before changing these controls.

`amd_rc` remains `vbr_latency` by default. A tight frame limit reduces large recovery bursts and the resulting send queue; it can reduce picture quality, especially during recovery or complex scene changes. [The historical 4K119.88 SDR comparison](../rust/PERFORMANCE.md#existing-amf-frame-size-limit-at-the-observed-pace) measured substantially smaller IDRs with a fixed 2× cap at 70.77 Mbps; it does not establish native HDR game quality or the quality of the new Automatic policy. Compare your content and select `0` for the driver's own behavior, or a fixed multiplier for a consistent budget.

Frame-size limits are best-effort: encoded frames can exceed them. If the driver rejects, ignores or clamps a requested cap during setup or a runtime update, the stream continues with its effective limit and reports an `encoder_frame_size` warning. Explicit peak-bitrate and VBV requests still report an AMF setting error if unsupported. Malformed or out-of-range values remain invalid. Explicit positive frame limits scale with host bitrate adjustments; Automatic recomputes from pacing, and `0` adds no frame-cap request even if the driver has a nonzero default. These settings affect AMD AMF only.

### Encoder focus

**Encoder focus** (`encoder_focus`, under **All settings → Picture → Encoder**) is off unless you choose it. On AMF, measured on the RX 7900 XT, a scene cut on a frame with a focus map came out about twice the size of the same cut without one, which is why it is a choice and not a default. `static` (Still areas) attaches a focus map (an AMF ROI map, or NVENC's QP delta map) that gives blocks which stayed still for a few frames, such as text and HUDs, more bits than moving ones; `centre` (Still areas and centre) also favors the middle of the screen and the cursor. The stream stays a normal H.264, HEVC or AV1 stream for every client. It runs only on codecs the startup encoder check verified on this GPU (the host log's `encoder focus check:` line); otherwise the stream shows the `encoder_focus` warning and focus stays off. There is no automatic value. App and device overrides work as for the other encoder keys. Design and measurement plan: [docs/encoder-focus-design.md](encoder-focus-design.md).

### NVIDIA NVENC

On an NVIDIA GPU, **Encoder → Automatic** encodes H.264, HEVC and AV1 with NVENC. Its settings are in the collapsed **NVIDIA NVENC** group of **All settings → Picture**, which shows while NVENC is the encoder in use (search finds them otherwise, as it does the AMD AMF ones), listed with their keys and defaults under [All settings, Picture](#picture); the stream presets set `nvenc_vbv_increase`, `nvenc_preset` and `nvenc_temporal_aq` as shown in [Stream presets](#stream-presets). **Rate control** (`nvenc_rate_control`) defaults to constant bitrate; with variable bitrate (`vbr`), **Peak bitrate** (`nvenc_peak_bitrate_ratio`, 1× to 3×, default 1.5×) sets how far busy scenes may go above the stream bitrate, at the cost of larger bursts on the network. **Full GPU clocks while streaming** (`nvenc_latency_over_power`, on by default) sets NVIDIA's power management to Prefer maximum performance in a driver profile that covers the host alone; turn it off to keep the driver's own power management. [NVENC](nvenc.md) describes how the host drives NVENC and how it matches the AMF path. The AMD AMF settings above do not apply to it.

### RTX HDR and RTX Video enhancement

Rubylight's separate NVIDIA build ([building](building.md#the-nvidia-build), unsigned; its files carry `nvidia` in their names) adds two NVIDIA image enhancements that run on the PC's GeForce RTX graphics card before the encoder. The signed build has neither, and their settings show only where the host offers them (elsewhere the console's settings search lists them as not available on this PC, without their controls): at start the NVIDIA build checks each one on the streaming GPU and reports what it offers in serverinfo's `NvidiaEnhancements` (bit 1 RTX HDR, bit 2 RTX Video enhancement). Where a check fails, the console's app page says why, and a stream that asks for an enhancement the PC cannot run gets a visible warning and streams without it.

- **RTX HDR** (`rtx_hdr`, default `false`, in **All settings → Picture → RTX HDR**) turns a game's SDR picture into HDR with NVIDIA's RTX HDR model when a device streams in HDR. It runs only for an app or device that turns it on; this switch alone converts nothing. The streamed display then stays in SDR, `prefer_sdr_10bit` does not apply, and the HDR picture uses `hdr_peak_brightness`. `rtx_hdr_sdr_brightness` (0 to 100, default `0`), `rtx_hdr_contrast` and `rtx_hdr_saturation` (−100 to 100, default `0`) and `rtx_hdr_middle_gray` (10 to 100, default `50`) tune it; a value in the game's own NVIDIA driver profile (set in the NVIDIA app) wins, polled every 250 ms while it runs.
- **RTX Video enhancement** (`rtx_video`, default `false`, in **All settings → Picture → RTX Video enhancement**) runs NVIDIA's RTX Video Super Resolution at the stream's own size, so it cleans up compression artefacts and sharpens without scaling. `rtx_video_intensity` (0 to 100, default `100`) maps to its four quality levels; 0 leaves the picture unchanged. It reads SDR captures and the 16-bit HDR capture, not the 10-bit one. When both are on for an HDR stream of an SDR game, RTX HDR runs.

Each app can turn either on or off and set its own values in the console's app editor (**NVIDIA image enhancement**), and a device's own per-PC choice in Nightjar overrides the app (launch parameters `rlRtxHdr` and `rlRtxVideo`). Both add GPU work to every frame while they run and nothing while they are off. NVIDIA's DLSS denoiser is not offered: [NVENC](nvenc.md#rtx-hdr-and-rtx-video-enhancement) says why.

## Displays and RTSS

Under **All settings → Picture → Streamed display**, select a physical display, or a virtual display for each device or one shared by all devices. **Virtual display layout** (`virtual_display_layout`) defaults to **Exclusive**, which switches the other monitors off. The **Extend** layouts keep the existing desktop active, with the PC's display or the virtual display as primary; the **Isolate** layouts also place the virtual display far from the other monitors, so the mouse cannot wander onto it.

A Remote Monitor follows the layout while it streams: exclusive turns the other displays off and makes it primary, the primary layouts make it primary, and it keeps the place the remote monitor layout gives it. Choose an Extend layout to use a remote monitor beside your own. A layout you pick for a device under Devices overrides the host setting, and the host never replaces it. A device without one of its own keeps the layout you leave a stream's display in, such as Windows' "Show only on 2", and uses it from the next stream; set it back to the host setting to forget that. A layout changed through the clients update API (`/api/clients/update`) counts as set by you; send `virtual_display_layout_set_by_user: true` to mark one without changing it.

**Windows HDR Calibration on virtual displays** (`virtual_display_hdr_calibration`, default `false`) gives an HDR stream's new virtual display the profile the Windows HDR Calibration app made for your HDR monitor (the primary one first), and gives the display its own profile back after the stream. It is off by default because the profile describes that monitor, not the device you stream to. A profile chosen for the device (`hdr_profile`) comes first; with none found, the host log says so and the stream stays uncalibrated.

Resolution and refresh policies default to `auto` (`dd_resolution_option`, `dd_refresh_rate_option`). Virtual refresh follows the frame-limiting policy unless a manual or device display mode overrides it. To leave a **physical** monitor's resolution, refresh and HDR unchanged, select Disabled for `dd_configuration_option`.

A device's **Display mode** (`display_mode`, `WIDTHxHEIGHTxREFRESH`) sets its virtual display's resolution and refresh while the stream keeps the size and frame rate the client asks for, so a game can render at 1440p on a 4K stream. To pick just the resolution from Windows instead, leave the display mode empty and turn on **Devices → (device) → Remember the resolution and scale I pick** (`remember_display_mode`): change the stream's virtual display resolution in Windows display settings during a stream, and when the device's last stream disconnects Rubylight keeps it (`remembered_resolution`), puts it back if the stream resumes, and creates the display at it from the next stream ([#36](https://github.com/RamazanKara/Rubylight/issues/36)). The Windows display scale you set on that display during the stream is kept the same way (`remembered_scale`, in percent, for example 200) and applied after the display is at its mode, so a 1440p display comes back at 200% too. A **Virtual display scale** set in the host settings (`dd_virtual_display_scale`, a percentage or Recommended) wins over a kept scale, and a scale the device asks for at launch (`rlDisplayScale`, Nightjar's **PC display scaling**) wins over both: then that scale is applied and no scale is kept. A scale the device asks for at launch that is too large for the display's resolution is lowered to the largest Windows offers there, with a warning, and one Windows refuses is a warning too: the stream goes on at the scale Windows shows. The device's **Display scale** set in the console is applied as set. A scale Windows refuses is logged as a warning and the stream goes on at the scale Windows shows, and only a scale Windows actually shows is kept. What is kept is the scale the display shows at the end, and Windows derives it from the recommended scale for the resolution, so a resolution change on its own (a game going exclusive fullscreen, say) can also be kept as a changed scale. The scale is put back when a paused stream resumes, so a change made at the PC while it was paused is not kept. The refresh is not kept: the stream's rate always wins, so the refresh follows the stream and **Virtual display refresh** as usual, including VRR's 1000 Hz, and a refresh Windows refuses falls back at the kept resolution. It applies to a device's own virtual display, not a shared one. Nothing is kept from a stream whose resolution Windows did not take, since the display then shows Windows' choice rather than yours. Turning the option off forgets the kept resolution and scale.

**Rotate the display** (`dd_rotation_option`, default `disabled`) gives the streamed display the device's portrait or landscape shape. Set it in **All settings → Picture → Streamed display** or for one device under **Devices → (device) → Display overrides**. **Match the device** (`auto`) makes a device's own virtual display in the stream's shape, so a phone streaming in portrait gets a portrait display; a virtual display is never turned, since a turned one would reach the encoder sideways. A monitor gets the stream's mode in the shape it shows, then turns to the stream's shape. When the device turns during the stream and asks for the turned size (Nightjar's Orientation → Rotate), the virtual display changes shape or the monitor turns, off the stream loop; a display that cannot follow logs a warning and the stream's picture is scaled to the new size. **Choose below** (`manual`) turns the display to **Rotation** (`dd_manual_rotation`: `0`, `90`, `180` or `270` degrees clockwise); a virtual display is made in the turned shape instead. A monitor is turned back when the stream ends, and after a crash when the host next starts. A monitor another stream is using keeps its rotation. A turned monitor needs Windows Graphics Capture: Desktop Duplication refuses it with an error. A device's own **Display scale** (`dd_virtual_display_scale` in its overrides) also scales a monitor and is put back the same way; the host-wide Virtual display scale sets only virtual displays.

A disconnect leaves the app running but restores its displays by default: `dd_config_revert_on_disconnect=true`, after `dd_config_revert_delay` (default `3000` ms). Exclusive layouts use that delay even in profiles that turn restoration off, so the physical monitors do not stay disabled while nobody is streaming; with `dd_hold_layout_while_paused=true` the user chose to keep them off, so with restoration turned off the lifetime below applies to them too, and a **Restore hotkey**, once set, still brings the monitors back. Other layouts can still be kept for reconnection by turning restoration off; `dd_paused_virtual_display_timeout_secs` then controls their lifetime (default `7200` seconds; `0` keeps them until the app closes). Restoration waits for the last stream using a layout. If that layout disabled an original display, the original layout is restored even after a Win+P change; other layouts preserve later user changes. A failed restoration remains in the recovery journal and the next stream retains its original baseline. A device remembers eligible layout changes only at its last stream's disconnect; later local changes during the pause do not become its streaming default. A client that sends nothing for `ping_timeout` (default `10000` ms) ends its stream. **Advanced → Troubleshoot** saves and restores a display layout and removes the virtual displays Rubylight created.

**Virtual display → Default (by Windows version)** leaves the choice to the host: one virtual display per device on Windows 11 23H2 or later, none on earlier Windows. **Activate a virtual display** (`dd_activate_virtual_display`) is shown while Virtual display is unset or Off, or while it is on. It gives streams a virtual display when the mode is off, and it also overrides a device's or app's own Off.

Under **All settings → Picture**, in the **Frame limiter** and **RTSS** groups:

- Keep **Limiter** on Automatic, or select RTSS. Set **RTSS folder** (`rtss_install_path`) only if detection fails; an empty value searches Program Files.
- Rubylight starts RTSS when a limit is needed and restores the previous limit after disconnect. If RTSS requires administrator access, the installed service can use the signed-in administrator's token; otherwise start RTSS with the required access yourself.
- **Frame limit** (`frame_limiter_fps_limit`) defaults to `0`, meaning the stream rate. RTSS preserves fractional rates such as 59.94 FPS.
- **Virtual display refresh** offers 2× (`legacy`), 4× (`enabled`), 1000 Hz (`vrr`) or Off (`disabled`). Turning **Limit every stream** off does not disable the automatic virtual-display limit. Choose **Limiter → None** (`frame_limiter_provider = none`) to disable all limiting.
- **RTSS limit mode** (`rtss_frame_limit_type`, default `async`) offers Asynchronous, Front edge sync, Back edge sync and NVIDIA Reflex. Virtual-display streams (unless **Virtual display refresh** is Off) and frame-generation games on a monitor choose their own mode: NVIDIA Reflex when the PC streams with an NVIDIA GPU, front edge sync otherwise. **Use this mode on virtual displays** (`rtss_allow_virtual_display_override`) applies your mode to virtual-display streams instead; apps set to game-provided frame generation keep front edge sync. On a GPU other than NVIDIA, NVIDIA Reflex limits with front edge sync and the log says so.

## Lower bitrate when the link cannot carry it

`link_rate_adaptation = true` (the default) lowers the encoder bitrate when the network cannot carry the stream, for example 150 Mbps requested on a 100 Mbps link. It acts only on loss that error correction cannot repair, so a link that carries the stream sees no change at all: no bitrate change, no pacing change, and no reaction to round-trip time alone.

- **What counts.** Three unrecoverable frames within two seconds; at least 5% of the packets the host sent reported missing in two consecutive half-second windows (FEC repairs less than that, so 0.5% random loss on Wi-Fi never triggers it); three frames the host could not send whole; or three network-caused keyframe requests from a client that sends no FEC status. A keyframe request caused by the decoder or by the host itself does not count. A client asks to invalidate frames again for every frame it cannot decode until the repair arrives; requests that continue the same frame range within half a second of the first count once. A client that announces FEC status (`x-ml-general.featureFlags` 0x01) is judged by its reports from the first frame, not by its requests; one that announces it but never sends a valid report gets no reaction to its requests either, except while a probe step is unconfirmed or after a pacing latch. Under those two, every repeated request counts.
- **What it does.** One cut to 85% of the rate the link delivered, never below half the current rate for a first cut. Reports about frames sent before the cut took effect are ignored, so the queue left over from the old rate cannot trigger a second cut. A second cut within two seconds of the first is not held to half (it goes to 85% of the delivered rate, no lower than 1 Mbps of wire rate), so a drop from 80 to 10 Mbps takes two cuts; a stream only a little above its link needs one.
- **Pacing follows.** Packet pacing follows the bitrate, so a cut also lowers the route pacing, and it rises again with each probe step. `keyframe_loss_pacing` is separate and slows pacing on its own evidence.
- **Coming back.** After two seconds the host raises the bitrate in steps of 12.5% (5% close to the rate that failed), and each step is confirmed after two seconds without evidence. A step that fails goes back to the last good rate and the next try waits twice as long, up to eight seconds. When steps keep failing at the same rate (a link with a fixed limit), the wait under that rate doubles up to 30 seconds and probes stay under 90% of it, so a fixed link pays about one failed probe every 45 seconds instead of one every 9. Probes are sent, and steps confirmed, only while the picture moves. After ten seconds at the requested bitrate the stream is back to normal.
- **What you see.** The first cut writes a WARN line to the host log (`the network to this device cannot carry the stream; lowering the video bitrate`, with the evidence and the old and new bitrate), later changes are INFO lines with `link_rate_change`, and the console's stream card shows "Bitrate limited by the network" with the current and requested bitrate while the stream is below the requested rate. The session API has a `link_rate` object (`state`, `ceiling_kbps`, `current_kbps`, `delivered_kbps`, `steps_down`, `probes`) and the five-second `stream timings` line carries the same as `link_*` fields. The `link_rate` object also has `bad_kbps` (the rate that last failed), `encoder_floor_kbps` and `last_change_ms_ago`. The warning gives the lowest bitrate of the episode ("as low as"), the card the current one. If even the lowest bitrate the host uses cannot be carried, the warning says so and tells you to check the network path.
- **An encoder that will not go lower.** Some encoders stay above the bitrate they are asked for on some pictures (H.264 on AMD sends about 14 Mbps on noisy content at 1080p60 and about 5 Mbps on milder content). When the stream stays more than 20% above the new target in two rounds, the host takes what the encoder sends as its floor, stops cutting below it, says so in the warning and on the card, and probes upward from there instead of cutting down to 2 Mbps.
- **PyroWave.** PyroWave streams have it too. Only a lost coarse block loses a PyroWave picture: detail blocks carry no parity while the picture moves, so a lost detail packet blurs part of one picture and counts only toward the loss share. Its keyframe requests count for nothing (every picture is intra), so it is judged by its FEC status alone. The cuts stop at PyroWave's severe-loss floor for the stream (the table above, 187 Mbps at 1080p60) instead of 2 Mbps, and when even that is too much the warning says to use HEVC or AV1 for the device. A stream at or below its floor (200 Mbps at 1080p120 is under the 373 Mbps floor) has nothing to cut: it keeps its bitrate and gets that warning once, saying this link loses PyroWave at that rate. A PyroWave client that sends no FEC status gets no reaction at all. The ceiling is the requested bitrate up to 2 Gbps (500 Mbps for the other codecs).
- **When it is off.** `link_rate_adaptation = false`, `host_adaptive_bitrate = true` (which controls the bitrate itself), and a session in which the client has set the bitrate through `/bitrate` (Nightjar's Automatic bitrate does).

## Adapt to network congestion

**All settings → Picture → Bitrate and network → Adapt to network congestion** sets `congestion_control = true` for the next connection. It is off by default. It needs a device that reports packet arrival times (Nightjar does, to a host that offers it) and a bitrate the device does not control itself: Nightjar's **Automatic bitrate** sets the bitrate through `/bitrate`, and then this stays off.

- **What it reads.** About every 50 ms the device sends the arrival time of each video packet since its last report (control message `0x5547`, [control messages](control-messages.md#0x5547-transport-feedback-20-bytes-and-the-entries)), a few hundred bytes. The host pairs each arrival with the time it sent that packet. When packets take longer and longer to arrive, a queue is building at the slowest link on the way, typically a Wi-Fi access point shared with a download, and the delay grows before anything is lost.
- **What it does.** It follows the delay-based and loss-based controller of the IETF's Google Congestion Control draft with WebRTC's constants: packets are grouped every 5 ms, a trend line over the last 20 groups says whether the delay is rising, and an adaptive threshold decides when that is overuse. On overuse the bitrate goes to 85% of what arrived, at most once a round trip and never below half at once. While the delay holds, the bitrate grows by 8% a second, and in small steps once it is near the rate that last failed. Loss above what error correction repairs (at least 10%) cuts it as well; lower loss is left to error correction. When packets arrive more slowly than they were sent, video is paced at that measured rate (with room above the bitrate), so bursts do not fill the queue.
- **Bounds.** It never raises the bitrate above the one requested (or `max_bitrate`) and never cuts below 2 Mbps (PyroWave's severe-loss floor for PyroWave). With **Lower the bitrate when the link cannot carry it** on as well, the lower of the two applies, and when both let go the requested bitrate comes back. If reports stop for two seconds, its limit and pacing are released.
- **What you see.** A cut writes a WARN line `congestion control` to the host log with the kind (`overuse` or `loss`), the old and new rate, the rate that arrived and the loss; a release is an INFO line. The stream card shows "Bitrate limited by the network" as for link-rate adaptation. The session API has a `congestion` object (state, target, arrived rate, loss, trend, queueing delay, cuts, recovery times), the five-second `stream timings` line has the same as `cc_*` fields with the queueing delay p50/p95 of those five seconds, and the end of the stream logs a `congestion summary`.
- **When it is off.** `congestion_control = false` (the default), `host_adaptive_bitrate = true`, a device that does not report arrivals, and a session in which the device has set the bitrate.


## Host bitrate adaptation

**All settings → Picture → Bitrate and network → Adapt bitrate on this PC** enables
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
feedback and wire budget. The controller does not promise lower latency on
every connection.

## Error correction chosen by the device

A device that supports it (Nightjar) chooses its own share of error-correction packets for each stream, a fixed percentage or automatic (`rlFecPercentage`), and reports its loss to the host. **Most error correction a device may ask for** (`fec_max_percentage`, default `50` %) is the most it may choose. Devices that choose nothing get the host's own adaptive error correction, as Moonlight clients always have. The automatic frame cap follows the share chosen for each frame, so a scene cut stays inside what that share can protect. On shaped Wi-Fi links the device's automatic share cut median loss from 2.37 to 0.26 % and freezes from 21 to 1 against the host's adaptive share ([October 27](../rust/PERFORMANCE.md#october-27-client-error-correction-and-a-link-test-before-the-first-bitrate-nuc)).

**Bandwidth test** (`bandwidth_probe`, default `true`) lets a paired device time a short download from the host (`GET /bandwidth-probe`) before it picks a bitrate. The device decides whether to run it; with the setting off the host refuses the download.

## Roaming

**Client roaming** (`client_roaming`, default `true`) lets a device whose address changes, such as a phone moving from Wi-Fi to mobile data, keep its stream. The device proves the new address with the stream key first; the host then sends there and drops what arrives from the old address. Only devices that support it (Nightjar) use it; other Moonlight clients stream as before. Off, the host neither offers it nor answers roaming datagrams. Turning it off applies at once; turning it on applies from the next stream.

## Prioritize stream traffic (DSCP)

**Prioritize stream traffic (DSCP)** (`dscp_tagging`, under **All settings → Network → Streams**, default `true`) marks the stream's packets for devices on the local network, when the device asks for it as Moonlight does there: video as audio-video traffic (DSCP 40, 802.1p 5) and audio as voice (DSCP 56, 802.1p 7), through Windows' qWAVE service. Wi-Fi sends marked packets from its video and voice queues, ahead of other traffic, and switches that honour the marks do the same. Devices outside the local network ask for no marks and get none.

Turn it off when sound crackles or the picture stutters with one router and not another: some consumer routers drop or delay marked packets. Off, the host marks neither video nor audio and asks Windows for no qWAVE flow, whatever the device asks for; the device's own marks on what it sends are its own setting. It applies from the next stream.

## Game-only audio

Game-only audio uses the normal audio stream and works with every client.

**All settings → Sound & controls → Sound → Audio to stream** defaults to **System audio**, which keeps
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
Unless the client asks to play sound on the PC too, the game's processes play
on a virtual playback device nobody hears (Steam Streaming Speakers, or
`virtual_sink`), as Windows' Volume mixer would set it per app; nothing else
changes device or volume. Without a virtual device the game stays audible on
the PC and the stream warns. A Nucleus Co-op game moves as its instance starts;
each co-op player's choice holds for their own instance, unless the instances
run one program (Windows keeps one device per program), which the stream says.
Windows doesn't move sound a game already opened, so a game that opened it
before the move plays on the PC until it next starts (the stream says so).
The game goes back to its own device when Rubylight closes it or the stream
ends; one that quit or outlived a host crash goes back as soon as it next
starts (the host watches programs start; if Windows can't tell it, the host
logs an error and looks every 2 s instead). The game's voice chat stays on the
PC's communications device; the stream's warning counts only sound on the
PC's default device or the game's own, and names voice chat as a cause when
the communications device is one of those.
Windows sets a game's device only while it runs, so uninstalling Rubylight
while a game still waits leaves it on the virtual device: pick its device in
Volume mixer, or use its **Reset**. The host keeps that list in
`audio-app-recovery.json` in its config folder (an unreadable one is moved to
`audio-app-recovery.unreadable.json`). Other independent
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
are not available.

## PCM and Dolby Digital audio

**PCM and Dolby Digital audio** (`audio_passthrough`, default `true`) offers
three formats besides Opus to clients that pick one, for TVs, soundbars and
receivers that decode the sound themselves. Stock Moonlight clients never see
the offer and keep Opus.

| Format | Channels | Rate |
| --- | --- | --- |
| PCM (16-bit, 48 kHz) | stereo, 5.1, 7.1, 7.1.4 | about 1.5 Mbit/s per stereo pair |
| Dolby Digital (AC3) | stereo, 5.1 | 192 or 640 kbit/s |
| Dolby Digital Plus (E-AC3) | stereo, 5.1 | 256 or 768 kbit/s |

Dolby Digital and Dolby Digital Plus are encoded by FFmpeg's libraries, which
the installer puts beside `butterpollo.exe`; without them the host offers PCM
only and logs why. Dolby frames are 32 ms of sound each, so they arrive a little
later than Opus. A client that asks for a format or layout this PC cannot encode,
such as Dolby Digital Plus 7.1, is refused with the reason when the stream
starts, and never gets other audio than it picked.

## Surround, night mode and headphone surround

**7.1.4 surround** (`audio_surround_714`, default `true`) offers 7.1.4 sound to devices that ask for it. The four height channels stay silent unless the captured playback device has them.

Two sound options work on the host's audio before it is encoded, and change nothing while they are off:

| Key | Default | What it does |
| --- | --- | --- |
| `audio_night_mode` | `false` | Evens out loud and quiet passages, so explosions stay bearable and voices stay clear at low volume. |
| `audio_headphone_surround` | `false` | For a stereo device, captures the PC's surround channels and renders them for headphones, so sound keeps its direction. |

Both can be set per device or app in their overrides, and switched during a stream from the couch menu or a native client. They are not in the console's settings.

## Keyboard, touch, pen and cursor

Under **All settings → Sound & controls**, in the **Keyboard** and **Mouse, pen and touch** groups:

- **Keyboard follows text fields** (`keyboard_follows_focus`, default `true`) opens the device's keyboard when a text field on the PC gets the focus and hides it after, on devices that support it. **Send the text around the cursor** (`text_input_surrounding_text`, default `false`) gives the device's keyboard the text around the cursor for its suggestions; it never sends text from password fields.
- **Keyboard layout** (`keyboard_layout`, default `client`) turns the device's keys into scan codes with the layout the device reports; `us` uses US English, or enter a Windows layout id.
- **Native pen and touch** (`native_pen_touch`, default `true`) passes pen and touch to Windows as pen and touch. **Touch** (`touch_mode`, default `client`) takes touches as the device sends them, as a mouse, or as a trackpad, with **Touch sensitivity** (`touch_sensitivity`, `100` %) and **Touch gestures** (`touch_gestures`, two-finger scroll and right-click, default `true`). **Pen barrel roll** (`pen_barrel_roll`, default `true`) passes on how far a stylus is turned, for pens that report it.
- **Mouse input method** (`mouse_mode`, default `auto`) chooses how this PC moves its mouse for a device. `virtual_mouse` plugs in a virtual USB mouse (relative motion, wheel with fine steps, horizontal scroll, five buttons, and absolute positions on a single display) that Windows and games read like a real one, polled every millisecond; `injection` uses Windows input injection, which some games and anti-cheat ignore and which raw input marks as injected. `auto` uses the virtual mouse when usbip-win2, the driver the setup installs with **USB audio and haptics**, is present, and injection otherwise. `vmouse` and `sendinput` are read as `virtual_mouse` and `injection`. When the virtual mouse cannot be plugged in, input falls back to injection and the log says why; with `virtual_mouse` chosen, the stream says so too. A game or device can choose its own method (see [Settings for one app or device](#settings-for-one-app-or-device)). [Virtual mouse](virtual-mouse.md) has the comparison with injection and how to measure it.
- **Touchpad gestures from devices** (`native_touchpad`, default `true`) offers devices the touchpad packets: a device with a physical touchpad (a tablet's keyboard cover, a laptop running Nightjar) sends its fingers, and this PC replays them on a virtual Windows precision touchpad, so Windows' two-finger scroll, three- and four-finger gestures and the touchpad settings work. It needs the same usbip-win2 driver; without it, or with the setting off, devices keep their touchpad as a mouse, and a device that sent fingers anyway gets a stream message.
- **Device-drawn cursor** (`client_cursor`, default `true`) lets a device draw the mouse cursor itself, so it moves without waiting for the video. It needs Desktop Duplication capture; with other capture the cursor stays in the video.
- `absolute_left_release_delay` (milliseconds, default `10`, at most `1000`; config file and device or app overrides only) holds a left-button release back that long after absolute mouse, touch or pen input, as Sunshine does, so a long press can right-click before the left click lands. Windows buttons act on the release, so every such click waits this long; `0` sends the release at once and gives up the long-press right-click.

## Controllers

**All settings → Sound & controls → Controllers → Controller type** controls which virtual controller games see. Standard profiles use the VHF gamepad driver included in the installer. Composite DualSense waveform haptics and native Steam Deck controllers use the optional usbip-win2 driver, bundled in the installer as **Steam Deck and DualSense USB audio and haptics**.

`gamepad = auto` matches each client's controller. PlayStation clients get a DualSense, the only pad with adaptive triggers; it also carries motion, battery, touchpad, lightbar and rumble. A game's trigger effects reach the player only if the client forwards them to a real DualSense. A client's Switch 2 Pro Controller gets a Switch 2 Pro Controller when usbip-win2 is installed (see below); other Nintendo clients get a Switch Pro. Other clients get an Xbox Series pad, or a DualSense when they have motion sensors or a touchpad and `motion_as_ds4` or `touchpad_as_ds4` is enabled. Both preferences default to enabled, so a Steam Deck reporting motion gets a DualSense.

`vhf_xbox`, `vhf_xbox_one`, `vhf_ds4`, `vhf_ds5` and `vhf_switch` give every client that pad: `vhf_ds5` a DualSense, `vhf_ds4` a DualShock 4 without adaptive triggers. Logs name the profile for each connected controller.

**Steam Controller** (`gamepad = vhf_steam`) gives every client a 2026 Steam Controller (`HID\VID_28DE&PID_1302`), which Steam drives as the real one: both trackpads, back grips, capacitive grip and stick touch, motion and battery. A client with two touchpads has one per pad; a single touchpad is split into a left and a right pad. A client that has a real Steam Controller and says so gets Steam's pad haptics and settings writes for it, and its explicit pad and trigger clicks and firmware attributes reach the virtual one; any other client gets rumble made from the haptics, and its virtual controller reports firmware Steam does not offer to update. `forward_rumble` withholds both kinds of haptics. A Steam Controller client keeps the virtual Steam Controller rather than a Steam Deck. Never start a controller firmware update from Steam while streaming: it would target the virtual controller, not the real one. The profile needs the virtual gamepad driver from this release; with an older driver the stream says the profile is unavailable.

**Xbox 360 (ViGEmBus)** (`gamepad = vigem_x360`) gives every client, Steam Decks and co-op and background streams included, an Xbox 360 pad on [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases), which you install yourself: it appears as a USB device (`USB\VID_045E&PID_028E`), for tools that look for USB controllers and may not see a VHF pad, which Windows lists only as a HID device. ViGEmBus is retired upstream, with no more updates, and this pad has no motion, touchpad or battery, so choose it only when a tool needs it. Without ViGEmBus the host never swaps in another pad: controllers stop working and the stream says why.

Earlier releases also offered Xbox 360 (`x360`) and DualShock 4 (`ds4`) through ViGEmBus. Rubylight changes those settings to `vhf_xbox_one` and `vhf_ds4` when it loads them, in the host configuration, app and device overrides, and configurations imported from Sunshine, Apollo or Vibepollo. ViGEmBus is used only for `vigem_x360`; otherwise an installed copy can be uninstalled.

### Switch 2 Pro Controller

A Nightjar client tells the host which controller it has (USB vendor and product). When that is a Switch 2 Pro Controller (`057E:2069`) and usbip-win2 is installed, Automatic attaches a virtual Switch 2 Pro Controller over USB/IP: the C button, both grip buttons (GL and GR), both sticks, gyro and accelerometer, battery, player lights and rumble back to the client. SDL 3 and Steam read it as the real controller, with Steam's own Switch 2 controller support. **Switch 2 Pro (USB/IP)** (`gamepad = usbip_switch2`) gives every controller one. Without usbip-win2, Automatic keeps the Switch Pro and the console says why; with `usbip_switch2` the controller does not work until the driver does. Moonlight and other clients, which do not say which controller they have, keep today's choice. Start the game after connecting the controller.

### DualSense waveform haptics

With a waveform-capable client and usbip-win2 installed on the host, Automatic can expose a DualSense with its matching USB audio endpoint. **DualSense with waveform haptics (USB/IP)** (`gamepad = usbip_ds5`) explicitly requires this mode. Games can then send their native actuator waveforms alongside ordinary rumble and adaptive triggers. See [setup and current limitations](dualsense-haptics.md).

**Haptics from game audio** (`audio_haptics`, off by default) feeds such a client's DualSense waveform haptics made from the game's bass and impacts while the game sends it none, with `audio_haptics_strength` (0 to 200 %), `audio_haptics_curve` (`linear`, `soft`, `punchy`) and `audio_haptics_gate` (-80 to -10 dBFS). It needs no USB/IP and stops as soon as the game vibrates the controller itself. See [haptics from game audio](dualsense-haptics.md#haptics-from-game-audio).

### Motion server for emulators (DSU)

**Motion server for emulators (DSU)** (`enable_dsu_server`, off by default) serves each streamed controller's gyroscope and accelerometer, buttons, sticks, triggers, touchpad and battery over the DSU (cemuhook) protocol, which Cemu, Dolphin, Citra, Yuzu and other emulators read for motion controls. In the emulator, choose DSU or cemuhook motion with server `127.0.0.1` and port `dsu_server_port` (26760 by default, 1024 to 65535). Controllers take slots 1 to 4 in the order they connect, from any streaming device, and leave them when they disconnect; a fifth controller gets none. The server starts with the host; changing either setting takes effect after a host restart. If the port is taken, the console says so and streams work without it.

The server listens on every network interface, as Foundation Sunshine's does, and has no password: any device that reaches the port can read the streamed controllers' input, and pad data goes to whoever asks for it. The Windows Firewall blocks other devices unless you add a rule for the UDP port; for an emulator on another PC, allow it only on a network you trust.

### Steam Deck

A Steam Deck can reach the host as a **real Steam Deck controller**: Rubylight serves the Deck's own USB controller (Valve 28de:1205) over USB/IP on the host's loopback address, and [usbip-win2](https://github.com/vadimgrn/usbip-win2) attaches it with its signed driver, so Windows sees a Steam Deck plugged into a USB port. Steam on the host then recognises a Steam Deck and applies the Deck's Steam Input layout, with both trackpads, the gyro, the back grips (L4, R4, L5, R5), the Steam and "…" buttons and rumble. Games see whatever Steam Input makes of it, as on a Deck.

- **All settings → Sound & controls → Steam Deck and back grips → Steam Deck controller** (`steam_deck_controller`): **Automatic** (`auto`, the default) attaches a Steam Deck when usbip-win2 is installed and Steam is running on the host when the controller connects; **Steam Deck** (`steam_deck`) attaches one whenever usbip-win2 is installed, even without Steam, where only SDL-based games see it; **Virtual controller** (`virtual_pad`) always uses the virtual DualSense or Xbox pad below.
- **Install USB audio and haptics support on the host.** The installer bundles usbip-win2 0.9.8.1 as the optional **Install USB audio and haptics support** component; install it when reconnecting USB devices is convenient. Setup can reconnect USB hubs and require a restart. Rubylight finds `usbip.exe` through its registered installation, `C:\Program Files\USBip`, or `PATH`. Nothing listens beyond `127.0.0.1`.
- If usbip-win2 is missing or the attach fails, the Deck falls back to the virtual pad, and the stream card says why when **Steam Deck** was chosen or an attach failed.

Moonlight passes on the Deck's own controls only when Steam Input is off for Moonlight on the Deck. With it on, Steam on the Deck turns the controls into a virtual pad first: the host still gets a Steam Deck with the buttons, sticks and triggers, but no gyro, trackpads or back grips, because Moonlight never receives them. On the Deck, open Moonlight's controller settings in Steam, choose to disable Steam Input, and reconnect. If the paired device's name says Steam Deck (SteamOS calls it `steamdeck`) and its controller arrives without a gyro, the stream card says this.

Moonlight sends trackpad touches from builds whose SDL reads the Deck's trackpads (SDL 3).

With the virtual pad (no usbip-win2, or **Virtual controller**), Moonlight's Steam controller with gyro gets a DualSense:

- **Gyro and accelerometer** are the pad's motion sensors.
- **Trackpads** share the pad's one touch surface: the left trackpad is its left half and the right trackpad its right half. Clicking the right trackpad is the touchpad click.
- **Back grips** (L4, R4, L5, R5): the virtual pads have no back buttons, so each grip presses what **All settings → Sound & controls → Steam Deck and back grips** sets for it: a face button, bumper, a fully pulled trigger, a stick click, Back, Start, Guide, a d-pad direction, the touchpad click or Share/Mute (`back_grip_l4`, `back_grip_r4`, `back_grip_l5`, `back_grip_r5`; values `a`, `b`, `x`, `y`, `lb`, `rb`, `lt`, `rt`, `l3`, `r3`, `back`, `start`, `guide`, `dpad_up`, `dpad_down`, `dpad_left`, `dpad_right`, `touchpad`, `misc`). They default to nothing, and pressing an unmapped grip shows a hint on the stream card. The same settings map an Xbox Elite's paddles (P1 is R4, P3 is L4, P2 is R5, P4 is L5) and a DualSense Edge's back buttons.
- **Steam and "…" buttons** become the PS button and the DualSense's mute button (the touchpad click on a DualShock 4), when Steam on the Deck lets them through to Moonlight.
- **Rumble and battery level** are passed on.

Setting `motion_as_ds4` and `touchpad_as_ds4` both off keeps a Deck's virtual pad an Xbox pad, without its gyro and trackpads.

## MoonDeck

[MoonDeck](https://github.com/FrogTheFrog/moondeck) works with Rubylight. It launches Steam games from the Steam Deck through Moonlight, with [MoonDeck Buddy](https://github.com/FrogTheFrog/moondeck-buddy) managing Steam on the host. You only need one host app for MoonDeck; you do not need to add each Steam game to Rubylight.

1. Install MoonDeck on the Steam Deck and [MoonDeck Buddy on the Windows host](https://github.com/FrogTheFrog/moondeck-buddy/wiki/Buddy-installation-guide). Keep Buddy running, pair it through MoonDeck's settings, and [pair Moonlight with Rubylight](getting-started.md#pair-a-device-and-start-desktop).
2. In Rubylight's **Games**, choose **Add manually** and add an app with **Name** set to `MoonDeckStream` and **Command** pointing to `"<BuddyInstallDirectory>\bin\MoonDeckStream.exe"`. Replace `<BuddyInstallDirectory>` with Buddy's actual installation folder and keep the quotes around the path.
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

## Couch menu

The couch menu is a host menu drawn into the stream, so any Moonlight client can reach it ([design](couch-menu-design.md)). It opens by holding L3+R3 on a controller, pressing Ctrl+Alt+Backspace, or holding the top-left corner. These settings, set in the configuration file or a device's or app's overrides, choose how it opens:

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

The menu offers Back to game, the [PC commands](#stream-menu-commands) the device may run, Disconnect (the game keeps running), End session and Close game. Each runs with the device's permissions as the console has them then, the same checks as the native API's session actions; End session, Close game and every PC command ask for a second press within 5 s. While the menu is open the game gets no input, and everything the pad, keyboard, mouse and touch held is released when it opens. B or Esc on the top page, the chord again, a tap outside the panel or Back to game close it. Nightjar and other native clients open and close it with `POST /api/native/v1/session/couch-menu` when the host lists `session.couch_menu`.

Input-only sessions and the read-back encode path (`wgc_direct_encoder_input = false`) have no picture the host draws on, so the menu stays closed there and the host log says why; so does a stream too small for the panel.

## Stream menu commands

A device with the **PC commands** permission (**Devices** → device → **Permissions**) lists commands it can run on this PC from its stream menu: Nightjar's **PC commands**, Artemis's server commands, and the rows above Disconnect in the [couch menu](#couch-menu). The list has two parts, in this order:

1. **PC commands from Settings** (`server_cmd`, under **Settings → Games → Commands from devices**): available in every app.
2. **The running app's own commands** (`menu-cmd`, the app's **Stream menu commands** panel): listed only while that app runs. Each has a name, a command line, an optional **undo** and **As administrator**. The app's commands run in its working folder with its environment, so they can use the same `$(SUNSHINE_…)` variables as its preparation commands.

An app's **Hide the PC commands from Settings** (`exclude-global-menu-cmd`, default off) lists only its own commands while it runs. An app with **Run devices' connect and disconnect commands** turned off (`allow-client-commands = false`) runs no PC commands at all.

**Undo.** When an app closes (quit from a device, End session, Close game, the game exiting or the host stopping), the undo of each of its menu commands that ran during that run runs once, the most recently run first, before the app's preparation undo commands. A command run twice is undone once. Commands from Settings have no undo: they belong to no app run.

A device runs a command by its place in the list, so a device that read the list before an app started or closed may be asked to reconnect: Nightjar and the couch menu check the name first and refuse a command whose place now holds another. A device may run one command a second. The couch menu asks for a second press within 5 s before it runs one.

The host checks an app's `menu-cmd` when it is saved: a list of at most 64 entries, each with a `name` and a `cmd`, an optional `undo` string and an optional `elevated` boolean. Apps imported from Foundation Sunshine keep their command `id`.

```json
"menu-cmd": [
  { "name": "Desktop pet", "cmd": "C:\\Tools\\pet.exe", "undo": "taskkill /im pet.exe", "elevated": false }
]
```

## Device connect and disconnect commands

Each paired device can run its own commands on this PC: **Devices** → device → **Commands**. **Connect** commands (`do`) run when that device starts streaming, **Disconnect** commands (`undo`) when its stream ends, including a stream that drops. Each command can run **As administrator** (`elevated`); otherwise it runs as the signed-in user. They run for every stream of the device, once per connection, without waiting for each other, and a disconnect command uses the environment its connect commands had, even if the app closed in between.

Commands can use `$(SUNSHINE_CLIENT_NAME)`, `$(SUNSHINE_CLIENT_UUID)`, `$(SUNSHINE_CLIENT_WIDTH)`, `$(SUNSHINE_CLIENT_HEIGHT)`, `$(SUNSHINE_CLIENT_FPS)` and `$(SUNSHINE_CLIENT_HDR)` (also as `APOLLO_…`), plus the running app's own variables. Typical uses: switch the room's lights or a TV input for one device, start a program only a handheld needs, change the PC's sound output for one device.

- **Run connect and disconnect commands** (the device's `allow_client_commands`, default on) turns a device's commands off without deleting them.
- **Run devices' connect and disconnect commands** in an app's **Behavior** (`allow-client-commands`, default on) skips every device's commands while that app runs; it also turns off [PC commands](#stream-menu-commands) for that app. Input-only connections (a second controller device) run none.
- The host checks both lists when they are saved, at most 64 commands each, each with a command line ([API](api.md)). A list in an imported or hand-edited state file that the host cannot read is reported in the host log, and the device streams without its commands, so no connect command runs without its disconnect command.

**Which commands run when.** Rubylight has five kinds of commands and one kind of notification; pick by the moment you need:

| Kind | Set in | Runs when | For |
| --- | --- | --- | --- |
| Preparation commands | `global_prep_cmd`, an app's `prep-cmd` | Before the app starts (`do`) and after it closes (`undo`), whichever device started it. A failing `do` stops the launch. | Getting the PC ready for any game: a display mode, a mod, a service. |
| Pause and resume commands | `global_state_cmd`, an app's `state-cmd` | When the app's stream resumes (`do`) and pauses (`undo`). | Pausing a game while nobody watches. |
| Device connect and disconnect commands | A device's `do` and `undo` | When that device connects and when its stream ends, for every app. | Things one device needs, whichever game it plays. |
| PC commands | `server_cmd`, an app's `menu-cmd` | When a device picks one in its stream menu; an app's undo runs when the app closes. | Actions on demand during a stream. |
| Detached commands | An app's `detached` | With the app, not waited for. | Helper programs that run beside a game. |
| Webhooks | **Advanced → Webhooks** | Devices pair, apps start and stop, streams begin and end ([Webhooks](webhooks.md)). | Telling another system; a webhook runs nothing on this PC. |

## Close apps on disconnect

**Settings → Games → Close the app when the last device disconnects** (`terminate_on_pause`, default `false`) closes the running app after its last streaming device leaves. When off, the app stays running so a device can reconnect. Input-only connections do not keep the app running. The setting takes effect the next time an app starts, including apps started before Windows sign-in.

Each app's **Behavior → Close the app when the last device disconnects** offers **Use global default**, **On** and **Off**. Existing explicit choices, including imported choices, still override the global setting; select **Use global default** to make such an app follow it again. Apps without a saved choice follow the global default, including newly added and automatically synced games.

Closing an app runs its normal shutdown and cleanup; unsaved progress may be lost. This setting controls what happens when a device disconnects. Ending the stream when a game exits remains governed by app process tracking, **Keep the stream when a launcher exits within 5 seconds** and **Wait for every process the app starts**.

## When a Playnite game closes

The stream of a Playnite game ends once Playnite reports the game closed and no program in the game's folder has run for **Wait after a game closes** (`playnite_exit_wait_secs`, default `15` seconds, under **All settings → Games → Playnite**). The wait keeps the stream open while a launcher hands over to its game or a game restarts itself; until it is over, the stream shows the desktop. Lower it to leave sooner, or set `0` to end the stream at the first check after the game closed; raise it if a stream ends while a game is still starting. It applies to the next game started.

The **Exit timeout** (`exit_timeout`) is a different wait: how long a game gets to close when Rubylight ends it, for example when you quit the app from the device.

## Playnite launch focus

When launching a Playnite game or fullscreen menu, Rubylight can bring its window to the front so controller input reaches it. Under **All settings → Games → Playnite launch focus**, **Confirm game focus** (`playnite_focus_attempts`, default `3`) sets the number of successful focus confirmations. Rubylight checks once a second, for at most **Stop focusing after** (`playnite_focus_timeout_secs`, default `15` seconds).

If launch keeps interrupting a window you are using on the host, enable **Stop after the first focus confirmation** (`playnite_focus_exit_on_first`, default `false`) or set either numeric limit to `0` to disable focus management. Stopping early lets a launcher or splash screen take focus back; increase the timeout when a slow launch otherwise leaves the game behind another window. Save and launch again to apply these settings.

## Radeon settings per game

An app can ask for graphics driver settings while it runs, on Radeon and on NVIDIA graphics cards. On the app's page in **Games**, **Graphics driver → Driver preset** picks a profile:

| Profile | Writes on Radeon | Writes on NVIDIA |
|---|---|---|
| Competitive | Radeon Anti-Lag on | Low Latency Mode at Ultra |
| Smoothness | Frame Rate Target Control on, at the stream's frame rate | Max frame rate at the stream's frame rate |
| Quality | Anisotropic filtering on and the driver's "enhance application settings" anti-aliasing | 16x anisotropic filtering and high-quality texture filtering |
| Custom | Up to 24 settings you list (ids as in `GET /api/native/v1/radeon`; a feature's `.enabled` toggle goes first) | The same |

Each preset lists both drivers' settings, and the host writes only those of the driver it streams with.

The table is `rust/core/src/radeon_profiles.json`. The app stores the choice as two top-level keys of the app, `radeon-profile` and, for Custom only, `radeon-settings` (an object of setting ids and values, such as `{ "anti_lag.enabled": true }`). The host refuses to save an unknown profile, `radeon-settings` with any profile but Custom (or Custom with none), an id that is not lower-case words joined by dots, a value that is not a boolean, a whole number or a short text, or more than 24 custom settings. At launch the host applies the settings through the Radeon control path of the Session API (validated, journaled, read back, undone on failure), and restores them with the other Radeon changes when the session ends or the host restarts after a crash. The app's config is the administrator's consent, so the device needs no Host control for its game's profile; the device's own Radeon writes (the Session API's panel) still do. The settings are written on a thread of their own, so a launch never waits for the driver, and a game launched from another device first has the earlier device's settings put back, so the new profile's originals are the PC's own. Starting another game on the same device puts back only the settings the earlier profile wrote, not what the device changed itself; Disconnect keeps the profile, and it goes with the session's end (or a launch that never connects). A setting the GPU does not list is refused before anything is written, a profile that follows the stream's frame rate is refused when the launch states none, and both are reported in the session's `radeon_profile.message` and in the host log; the game still starts.

## Managed game files

Some games and mods keep a frame limit or similar setting in an `.ini` file of their own: OptiScaler's `FramerateLimit`, for example. An app can have the host set such keys for a stream and put them back when the session ends. On the app's page in **Games**, the **Managed game files** panel lists them as rows of file, section, key and value. A device can have its own rows in the **Per device** panel ([one game on one device](#one-game-on-one-device)).

The app stores them in `game-files`, a list of up to 32 entries:

```json
"game-files": [
  { "path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": "{stream_fps-1}" }
]
```

**What the fields hold**

- **`path`.** An existing `.ini` file. A relative path starts in the app's working directory, else in its program's folder.
- **`section`.** The name between `[` and `]`. Leave it empty for keys before the first section.
- **`value`.** Fixed text or a number, or `{stream_fps}`. `{stream_fps}` is the stream's frame rate rounded to a whole number, as for RTSS. It takes an optional whole offset, so `{stream_fps-1}` writes 119 for a 120 fps stream.

**What the host changes**

Only the listed keys change. Every other byte of the file stays as it was, including line endings, a byte order mark, comments and key order. Files in UTF-8 or UTF-16 with a byte order mark are read. Section and key names are matched without regard to case, and the first match counts.

If the file, the section or the key is missing, the host logs a warning and the game still starts. The host never adds a key.

When a frame-limit key (a name with `fps`, `framerate` or `limit`) gets a value other than the host's RTSS frame limit, the host logs a warning: two limiters at different rates cap the game at the lower one.

**When**

The keys are written for a stream from a device, just before the game starts. A launch from the console writes none. The original values go back:

- when the session ends: End session, Close game, or the disconnect grace period running out;
- when the device is unpaired or disabled;
- when the host shuts down;
- at the next start after a crash.

Disconnect keeps them for the suspended session.

**How the restore works**

Before the file is written, the original value of each key and the value written are recorded in `game-files-journal.json` in the config folder. A restore puts the original back only while the key still holds the host's value. A value you changed in the meantime (in the game's own menu, for example) is yours and stays. Restores run on a thread of their own, so a slow file never holds up a session. A restore that fails stays in the journal and is retried: within 5 seconds of a sign-in when nobody is signed in to Windows, otherwise after 30 seconds and then less often, and again at the next start. A game folder deleted since takes its entries with it. When another device's stream changed the same key and its restore is still pending, that restore runs before the new stream writes.

**Which files the host writes**

The host runs as SYSTEM, but it reads and writes game files as the signed-in Windows user, so a link put in a game's folder cannot lead it to a file the user could not change. With nobody signed in, nothing is written, and restores wait. A file the user may not change (often the case under `Program Files`) is skipped with a warning. It writes only:

- an existing `.ini` file of at most 1 MiB;
- inside the app's working directory, its program's folder or its Steam install folder, after every link and junction is resolved.

It refuses:

- `..` in a path;
- network and device paths;
- a link or junction that leads outside these folders;
- a drive's root or the Windows folder as an app's folder.

The host checks the files when the app is saved and again at every launch. While it reads and writes a file, it holds the folders from the app's folder down to the file open, so none can be swapped for a link meanwhile. A file is written to a temporary file beside it and swapped in, so the game never reads a half-written file, and the file keeps its permissions and attributes. A read-only file is not changed.

## Game settings per device

A game's own settings files, such as its resolution and quality, can be kept for each device: the TV keeps its 4K settings and the handheld its 1080p ones ([#72](https://github.com/RamazanKara/Rubylight/issues/72), [#69](https://github.com/RamazanKara/Rubylight/issues/69)). On the app's page in **Games**, the **Game settings per device** panel lists the files with **Add settings file**. The app stores them in `game-settings-files`, a list of up to 16 full paths, such as `%LOCALAPPDATA%\Game\Saved\Config\GameUserSettings.ini`. `%LOCALAPPDATA%`, `%APPDATA%`, `%USERPROFILE%` and other variables of the signed-in Windows user work. A path starts with a drive letter; `..`, folders, network and device paths are refused when the app is saved. The host does not find a game's settings files by itself.

- **At launch.** When a device starts the game, the host keeps the PC's own files and puts that device's saved copy in their place. A device's first launch keeps the files as they are.
- **When the app stops** (End session, Close game, the game exiting), the files, with whatever the game changed, are saved as the device's copy and the PC's own files go back. While one of the game's processes still runs (a launcher kept it open, or the host restarted), nothing is put back until it closes, or until you press **Put back now** in the panel. An app that starts its game with detached commands puts the files back when the stream ends, even if the game still runs.
- **Failures.** A missing, locked or unreadable file, or one larger than 16 MiB, stops the launch with the reason before the game starts. A file cannot be both a [managed game file](#managed-game-files) and a per-device settings file.
- **Where the copies live.** The PC's files and every device's copies are kept in the `game-settings` folder of the host's config folder, with a journal that lets the host finish a swap a crash interrupted. A device's previous copy stays beside its new one, and the last three PC files put back stay as backups. The panel shows which devices have a copy.

The host reads and writes the files as the signed-in Windows user, as for managed game files.

## Neural Rendering

**Neural Rendering** is a switch on a game's page in **Games**. It installs a third-party neural upscaling mod into the game's folder, for games with DLSS, FSR or XeSS, and removes it again. It is off for every game.

- **Requirements.** Radeon graphics with RDNA 3 or RDNA 4 and 12 or more compute units, on the GPU Rubylight streams with, and AMD Software 26.9.1 or newer. Elsewhere the switch says why it is unavailable.
- **First use.** A notice explains that the mod is not made or endorsed by AMD or NVIDIA and that an online game may still treat a mod as cheating; **Cancel** is the default. The switch turns on only while nothing is streaming.
- **Download.** The host downloads one pinned release of the mod from its authors' GitHub release, about 1 GB (88 s on the test PC), and checks it against the pinned and the published checksums. It never updates the mod by itself. [Privacy](privacy.md#third-party-services) lists what GitHub receives.
- **Refused games.** Games with a known anti-cheat in their install, games Steam lists Valve Anti-Cheat for, and games that already carry another upscaler mod.
- **Where it goes.** The mod goes beside the game's own program, never a launcher's. When the host cannot tell which program is the game, the page asks for the game's `.exe`.
- **Removal.** Turning the switch off, or deleting or uninstalling the game, removes only the files Rubylight put there. A folder on a drive that is not connected is retried every 10 minutes.

## Background streaming

Background streaming lets a device play while someone keeps using the PC, for
example a child gaming on a tablet while a parent works at the desk
([#43](https://github.com/RamazanKara/Rubylight/issues/43)). Turn on
**Devices → (device) → Play in the background → Stream in the background**
for every stream of that device (`background_stream`), or **Games → (app) → Behavior → Stream in the
background** for one app (`background-stream`). Either one is enough; it applies
from the next launch.

Nucleus Co-op runs every background stream: the game runs as a
[Nucleus Co-op](nucleus-coop.md) instance whose window is captured on its own,
away from the PC's monitors. There is no mode to choose; a `background_mode`
line an earlier build wrote reads as Nucleus Co-op. A background launch
Nucleus Co-op cannot run, such as a game with anti-cheat, is refused with the
reason and never streams the PC's monitors instead.

A background stream:

- **Leaves the PC's displays alone.** The PC's monitors, layout and primary
  display stay as they are, and the PC user keeps the keyboard and mouse
  focus.
- **Accepts only controllers from the device.** Its keyboard, mouse, touch and
  pen would type and click into whatever the PC user has focused, so they are
  off, as is the clipboard. The virtual controllers work as usual.
- **Streams only the game's sound** ([Game-only audio](#game-only-audio)), so the
  PC user's calls and music stay private. The game plays on a virtual device,
  so it stays off the PC's speakers, even when the device asks to play audio on
  the PC: that setting still applies to its normal streams. Without a virtual
  device the game stays audible on the PC and the stream warns.

A game needs no handler from Nucleus Co-op's handler hub for this: without
one, the host writes its own one-player handler. The app's command must be the
game's .exe or a Steam link; a Steam game starts with its Steam app id, so it
doesn't restart through Steam, and a game the Epic Games Launcher installed
starts with the arguments that say which game it is, without its sign-in, and
none of its launcher's own options (the app's arguments can add them). A game
that insists on that sign-in may still restart on the PC's monitor, and the
stream says so. Other launchers' links, launchers' .exe files,
games with anti-cheat and games on a network share are refused, and some games
pause anyway when their window is not
in front ([Games without a handler](nucleus-coop.md#games-without-a-handler)).
Co-op still needs a hub handler. Nucleus Co-op needs the host running as the
Windows service; co-op, hub handlers and games that need administrator rights
also need an administrator account signed in on the PC
([Without administrator rights](nucleus-coop.md#without-administrator-rights)).
[Nucleus Co-op](nucleus-coop.md) has the install, the limits, and co-op for
several devices at once. Two settings under
**All settings → Games → Play together** shape it:
**Nucleus Co-op folder** (`nucleus_path`, the folder you unpacked Nucleus
Co-op 2.4.2 into, checked at each launch; empty: an earlier Rubylight's
install, if any) and **Co-op players** (`nucleus_coop_players`, default
`4`; the game's handler and the free controller slots can allow fewer). Which
devices may join a co-op game is the per-device **Can join co-op games**
(`coop_join`); a device that may launch apps may join until you take it away.

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

## Guests from other devices

A second paired device that joins the game another device plays (Nightjar's **Join**, or a Moonlight client starting or resuming the running app) becomes that device's guest: the same picture and sound with an encoder of its own, its own controller, and the keyboard and mouse only if granted ([Guests from other devices](multiclient.md)). Two settings under **All settings → Games → Play together** shape it:

- **When another device joins the running game** (`guest_join`, default `off`). **Off** streams it beside the game with its device permissions, as before guests: one person moving between their own devices needs no answer. **Ask the player** waits up to 45 seconds for the device playing, or the console's Home, to let it in. **Let in without asking** admits it at once.
- **What a guest may do** (`guest_permission`, default `controller`). **View only**, **Controller**, or **Controller, keyboard and mouse**. This is used when nobody chooses for the guest; the player can change it for each guest.

Both apply to the next join. Guests changed in the console or Nightjar take effect at once.

## Settings for one app or device

Open an app in **Games** or a paired device in **Devices** to set its display, HDR and other overrides. Leave an override unset to inherit the host configuration.

For general configuration overrides, Rubylight applies **host settings → device overrides → app overrides → the app's overrides for that device**. Only supported stream, input, display and encoder keys are accepted; host-wide network, identity and path settings cannot be overridden per stream.

Display selection has dedicated rules: a device's explicit virtual-display mode takes priority over the app's mode, and its **display mode** (`WIDTHxHEIGHTxREFRESH`) overrides the host's resolution/refresh policy. This does not change the frame rate requested for the encoded stream.

### One game on one device

An app can have settings of its own for one paired device, such as a 60 fps frame limit on the TV and 120 on a handheld. On the app's page in **Games**, the **Per device** panel adds a paired device and sets its **Stream preset**, **Frame limit** (`frame_limiter_fps_limit`, 0 for the stream's frame rate), **What to stream** (`capture_window`: the display or [the game's window](#the-apps-window-in-place-of-a-display)), **Radeon preset** and [managed game files](#managed-game-files). "Same as the game" leaves the app's own setting in place.

The app stores them in `device-overrides`, an object keyed by the device's id (its uuid in `GET /api/clients/list`). Each block holds the app's own keys for that device: `config-overrides`, `radeon-profile` with `radeon-settings`, and `game-files`.

```json
"device-overrides": {
  "0123abcd-...": {
    "config-overrides": { "stream_preset": "reference", "frame_limiter_fps_limit": 60 },
    "radeon-profile": "smoothness",
    "game-files": [ { "path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": 60 } ]
  }
}
```

The block wins everywhere the host resolves a stream's settings: **the app's block for the device → app → device → host**.

- **General settings.** A key of the block's `config-overrides` replaces the app's; an empty value leaves the app's.
- **Radeon profile.** A `radeon-profile` in the block, `""` for Off, replaces the app's profile together with its settings.
- **Game files.** An entry replaces the app's entry for the same file, section and key; the block's other entries are added.

The host checks a block when the app is saved, as it checks the app. It refuses:

- a key a block cannot hold;
- a host-wide setting, such as a port or a path;
- an unknown preset or Radeon profile;
- a game file entry the host would not write.

A block for a device that is not paired, or no longer paired, is kept and ignored. The console shows it as such, and it applies again if a device with that id pairs.

### Display choice from the client

A client can choose, for one stream, what the PC's displays do. [Nightjar](https://github.com/RamazanKara/nightjar) sets it per PC under **PC settings… → Display on the PC**. The client sends `hostDisplay` with `/launch` or `/resume`:

| `hostDisplay` | What the stream gets |
| --- | --- |
| `exclusive` | A virtual display, and the PC's other displays switch off for the stream. |
| `extended_primary` | A virtual display beside the PC's displays, made the primary display so games open on it. |
| `extended` | A virtual display beside the PC's displays; the primary display stays as it is. |
| `extended_isolated`, `extended_primary_isolated` | The isolated layouts of **Virtual display layout**. |
| `physical` | No virtual display; the stream shows the physical display the host and app settings select. |
| missing, `default` or anything else | The host decides from its settings, as for an older client. |

The choice comes before the host, device and app settings for virtual display mode and layout, including a device's **Always use a virtual display**. `virtualDisplay=0`, which some clients send whenever their own virtual display option is off, is not a choice and follows the host settings, as an omitted `virtualDisplay` does. Two cases still get a virtual display after `physical`: a PC with no active display, and an output that names the virtual display itself. A virtual display needs its driver; without it the stream shows the physical display with a warning, as for any other request. When another stream already holds the display layout, a second stream joins that layout. The layout from before the stream comes back when the stream ends, as with the host's own settings. A display kept for reconnection is reused only when the next launch asks for the same choice, or asks for nothing. Remote Monitor and Remote Input ignore the parameter. Older Rubylight hosts and other hosts ignore it, so Nightjar also sends `virtualDisplay=1` with the virtual choices, which those hosts honour where they support a virtual display.

## Host control, session actions and Radeon settings

A paired client such as [Nightjar](https://github.com/RamazanKara/nightjar) uses these through the [native API](native-api.md). Stock Moonlight and Artemis do not use them and keep working as before.

### Host control for a device

**Host control** (`host_control`) lets one paired device close games and change graphics driver settings (Radeon or NVIDIA). It is not part of the other permissions. Open **Devices**, open the device, then under **Permissions → Custom permissions → Other** tick **Host control: change graphics driver settings, close games, install a Rubylight update and manage guests from this device** and save.

- It is off for every device, including new pairings and devices imported from Apollo or Vibepollo. Only the console turns it on; nothing on the streaming port can.
- Turning it off applies on the device's next request. It does not end a running stream.
- A device also needs the **List apps**, **View streams** and **Launch apps** permissions for what it does. Reading the graphics driver state needs View or Launch only.

### Sleep PC and sleep after idle

**Sleep PC** (`allow_sleep`, off for every device) is set in **Devices → device → Custom permissions → Other**. It lets that device put this PC to sleep through Vibepollo's `/power-actions/v1` requests while nothing is streaming or running: a single-use token valid for 30 s, then the suspend once the answer is sent and the connection closed. It needs a PC with S1, S2 or S3 sleep, as most desktops have. Windows has no call to enter Modern Standby, so on a PC with only Modern Standby the host advertises no Sleep PC, refuses the request, and the console shows why instead of the setting; the host log says so at startup and names the sleep states the PC reports. The console grants it as `true`; a device imported from Vibepollo with `allow_sleep: true` keeps it.

**Sleep after idle** (`sleep_after_idle_minutes`, under **All settings → This PC → Sleep and wake**, default `0`, off) puts the PC to sleep when nobody has streamed or used it for that many minutes, 5 to 1440. **Sleep with a suspended stream** (`sleep_while_suspended`, default `false`) also lets it sleep while a stream is suspended or an app launched from a device still runs. A paired device can wake the PC again with Wake-on-LAN; [Privacy](privacy.md#your-own-devices) lists what it reads for that.

### Lock Windows when the last device leaves

**Lock Windows when the last device leaves** (`lock_on_last_disconnect`, under **All settings → This PC → Privacy**, default `false`) locks the signed-in Windows session once no device has been connected for 10 seconds: no stream, launch, browser stream or controller session. The PC is then not left signed in for whoever sits at it, as with Parsec's privacy mode.

- It never locks during a stream. A device that moves the stream with **Continue here**, roams or reconnects within the 10 seconds keeps the PC unlocked, and the host checks again right before it locks.
- It locks once each time the last device leaves, and only after a device was connected: starting the host or turning the setting on locks nothing.
- The lock runs as the signed-in user in their session (`LockWorkStation`). With nobody signed in there is nothing to lock.
- To get back in, start a stream: the lock screen streams like the desktop, so the device unlocks the PC with the Windows password or PIN ([Features](features.md#capture)). Signing in at the PC works as always. A game left running keeps running behind the lock screen.

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

On a host that streams with an NVIDIA graphics card, the same panel and the same restore cover NVIDIA's global 3D settings, read and written through NVAPI: Low Latency Mode, Max frame rate, Vertical sync, Power management mode, Texture filtering quality, Anisotropic filtering and Smooth Motion (driver 571.86 or newer). Nightjar shows them in its stream menu, and a game's **Graphics driver** panel in the console offers them for that game's [driver preset](#radeon-settings-per-game). A value the panel cannot put back exactly, such as one set with NVIDIA Profile Inspector, shows as read only and is left alone.

## Clipboard, files and USB

Each of these needs a permission you give a device on **Devices**; pairing never grants them, and **Everything** leaves the file, folder and USB ones as they are.

- **Clipboard.** A device with the clipboard permissions copies text to and from this PC, and images too while **Copy images** (`clipboard_image`, default `true`) is on, up to **Largest image** (`clipboard_image_max_mb`, `16` MiB as PNG) in each direction. Background streams have no clipboard.
- **Files and shared folders.** See [File transfer](#file-transfer) and [Shared folders](#shared-folders) below.
- **USB devices.** **USB forwarding** (`usb_forwarding`, default `false`, under **All settings → Sound & controls → USB devices**) lets devices with **USB devices** attach the USB devices plugged into them to this PC during a stream, up to **USB devices at once** (`usb_max_devices`, default `4`). It needs usbip-win2 on the host; [USB forwarding](usb-forwarding.md) has the setup.

[Privacy](privacy.md#your-own-devices) describes what each permission lets a device read or change.

## File transfer

Off by default. With **File transfer** (`file_transfer`) on, paired devices can send files to this PC and download them again over the streaming HTTPS port ([Native API, Files](native-api.md#files)). Each device also needs **Upload files** or **Download files** under its permissions on **Devices**; no device has them until you tick them, and **Full control** leaves them as they are. Files are written as the Windows user who is signed in; with nobody signed in, transfers are refused.

| Key | Default | What it means |
| --- | --- | --- |
| `file_transfer` | `false` | Turns file transfer on. Applies to the next request. |
| `file_transfer_dir` | empty | The folder uploads go to and downloads come from, as an absolute path. Empty means `Downloads\Rubylight` of the signed-in user. |
| `file_transfer_max_mb` | `4096` | The largest upload in MiB, refused before it starts. |
| `file_transfer_stream_mbps` | `50` | While a stream runs or starts, all uploads and downloads together move at this many Mbit/s. `0` lets them run at full speed. Applies to running transfers within a second. |

## Shared folders

None by default. **Shared folders** (`shared_folders`, under **All settings → Network → Shared folders**) lists folders of this PC that paired devices may browse, download from and upload to over the streaming HTTPS port ([Native API, Shared folders](native-api.md#shared-folders)). **Add folder** adds a row with a name, which devices show, and a path, typed or picked with **Browse**. The path must be a folder on a drive of this PC, such as `D:\Games`: a network share, a relative path, `.` or `..` is refused. The console shows what is wrong beside the field (no name, two folders with one name, a path the host refuses) and saves nothing until it is fixed.

Each folder gets an id when it is first saved, from its name and four random letters, such as `games-k2fq`; devices' grants name it. Renaming a folder or changing its path keeps the id, and so what devices may do with it; a folder removed and added again gets a new id, so it is shared with no device until you share it again. The setting is stored as a JSON list such as `[{"id": "games", "name": "Games", "path": "D:\\Games"}]`, where `id` is 1 to 64 letters, digits, `-` or `_`; to edit it by hand, keep each id. A list that isn't valid is reported in the log and shares nothing, and the console says it can't read it.

A device sees a folder only once you share it with that device: on **Devices**, open the device's **Settings**, then **Shared folders**, and choose **Read** (browse and download) or **Read and write** (also upload) for each folder. **Full control** and the file transfer permissions don't share any folder. A grant for a folder no longer shared is listed as no longer shared, and **No access** removes it. Taking a folder away from a device, or write access to it, disconnects it and ends its running transfers. File transfer doesn't need to be on, but its size limit and speed while streaming apply. Files are read and written as the Windows user who is signed in, only inside each folder: links and junctions inside it are neither listed nor followed.

## WebRTC server

Off by default. With **WebRTC server** (`webrtc_server`) on, Rubylight listens for TVs and browsers that speak the Moonlight WebRTC protocol (version 2). They can find the PC, pair, sign in, list its apps and stream. [WebRTC server](webrtc.md) has the setup, what streams and the protocol extensions for client authors.

| Key | Default | What it means |
| --- | --- | --- |
| `webrtc_server` | `false` | Turns the server on. Needs a restart; **Turn on** on the console's **Stream in the browser** page saves it and restarts in one step. |
| `webrtc_port` | `8000` | TCP port of the WebSocket, announced in the discovery reply. 1024 to 65535. Discovery itself always listens on UDP 8000, where TVs send their broadcast. |
| `webrtc_media_port` | `8001` | UDP port for WebRTC video, audio and input. 1024 to 65535. Guests of the console's stream use the next four ports ([guests](browser-guests.md#what-must-be-reachable)). |

- Pairing: the TV shows a four-digit PIN and the request appears on **Devices** next to Moonlight's, with a WebRTC badge. Enter the PIN there. The TV then holds a client ID and a secret; the secret is stored in `sunshine_state.json` and is never shown in the console. Paired TVs are listed on **Devices** with a WebRTC badge, where you can disable or unpair them. A disabled or unpaired TV is disconnected at once. At most 32 TVs stay paired; the oldest go first.
- Signalling is plain `ws://`, not encrypted. Signing in keeps other devices from using or interrupting a stream; it does not hide what is sent on the network. Only peers on the local network are served: loopback, private (RFC 1918), link-local, unique-local IPv6 and carrier-grade NAT (100.64.0.0/10) addresses. Anything else is closed before its first message and logged. Do not forward these ports on your router.
- At most 8 connections may sit unauthenticated at once (the oldest is closed for a ninth) and 32 paired devices connected.
- If another program holds the port, the console shows a `webrtc_server` warning, the log has the error and the server does not run. The port never moves on its own.
- `--bind` applies to both the WebSocket and the discovery socket. The installer's Windows Firewall rule allows `butterpollo.exe` inbound on every port, so TCP 8000, UDP 8000 and UDP 8001 (or the ports you chose) need no extra rule.

## Play over the internet

Devices away from home reach the PC in this order: its IPv6 address, a forwarded IPv4 port, then a direct path both sides punch through their routers ([Play over the internet](internet-play.md)). On the local network nothing changes.

**Automatic port forwarding** (`upnp`, **All settings → Network → Discovery and internet**) asks the router to forward every streaming port, through UPnP, PCP or NAT-PMP, and to open the IPv6 firewall for them through UPnP or PCP. It renews the mappings while the host runs and removes them when it stops. **Internet status** (**Settings → Network**) shows what the router mapped, each port it refused and why, and the public address. Stock Moonlight reaches a forwarded port at the PC's public address, which paired serverinfo reports as `ExternalIP`.

**Play over the internet** (`internet_play`) lets Nightjar reach the PC where no port can be forwarded, such as behind carrier-grade NAT. The host finds its public address through public STUN servers and meets each paired device through a public peer-to-peer directory (the Mainline DHT), where it leaves only encrypted notes. Nothing is downloaded or shared through it, and neither the project nor anyone else runs a server for it ([Privacy](privacy.md#third-party-services)). A device learns the way once while it is on the PC's network. Two routers that both change ports per destination cannot be punched; then only a forwarded port or IPv6 reaches the PC.

| Key | Default | What it means |
| --- | --- | --- |
| `upnp` | `false` | Automatic port forwarding through UPnP, PCP or NAT-PMP, and IPv6 pinholes. |
| `internet_play` | `false` | Play over the internet for paired Nightjar devices. Needs a restart. |
| `internet_play_stun_servers` | empty | STUN servers for the public address, `host:port` separated by commas or spaces. Empty uses `stun.moonlight-stream.org:3478` and `stun.cloudflare.com:3478`. Needs a restart. |

## Multiple instances

Give each instance its own profile folder and base port. An explicit `--config-dir` takes precedence over the portable defaults. From the extracted package, run these commands in separate PowerShell windows, one in each intended Windows session:

```powershell
.\butterpollo.exe --config-dir C:\RubylightProfiles\Seat1 --port 48123
.\butterpollo.exe --config-dir C:\RubylightProfiles\Seat2 --port 48323
```

The consoles are at **https://localhost:48124** and **https://localhost:48324**. These base ports also keep the instances' streaming ports separate from each other and from the default installed host. To make the port choice persist when using **Start Rubylight.exe**, save `port = 48123` or `port = 48323` in the corresponding profile's `sunshine.conf`. The launcher accepts `--config-dir`; `--port` is a host option.

For copied portable packages, create a separate `config` folder beside each copy of `butterpollo.exe` and set a different `port` in each folder's `sunshine.conf` before launching. Without an adjacent folder or explicit path, copies running as the same account share the per-user default, including SYSTEM's systemprofile location. Start with fresh profiles for independent identities and pair each instance separately; copying a populated profile also copies its identity and paired devices.

For multiseat use, launch each process in its intended Windows session through your multiseat software, using that session's SYSTEM token if required by your setup. The installer manages one Windows service for Rubylight, whether it [replaces another host or installs alongside it](getting-started.md#keep-another-host-and-install-alongside). Separate-profile startup has been smoke-tested with two simultaneous hosts.

## Updates

Automatic **checking** is on; automatic **installation** is not. **Settings → Updates** shows the release status, checks on request (**Check now**) and installs an offered update. **Include pre-releases** (**All settings → This PC → Updates**) also offers release candidates.

If you opt into **Install updates automatically**, the installed Windows service waits for one minute without active streams, pending connections, remote-monitor sessions or running host apps. The updater validates the selected installer against the release's size and SHA-256. Portable installations use the release download page instead.

**Check for updates** (`update_check_interval`, default `86400` seconds, at least `3600`) sets how often the host checks. Set it to `0` to disable scheduled checks and automatic installation; manual checks remain available.

To check at a time of day instead, set **Update check time** (**All settings → This PC → Updates**, `update_check_time`) to a time on the 24-hour clock such as `03:00`. The host then checks once a day, at the first moment it is free at or after that time on the PC's own clock, and with **Install updates automatically** on a new release installs after that check under the same idle rule. If the PC is off, asleep or streaming at that time, the check runs as soon as it can afterwards, not a day later. The time follows the PC's time zone and daylight saving time. While a time is set the interval's length is not used, but `update_check_interval = 0` still turns scheduled checks off. A value that is not a time (`3am`, `25:00`) is reported in the host log and ignored, and the interval decides. An empty value, the default, keeps the interval.

## Settings backup and restore

**Advanced → Troubleshoot → Settings backup** saves the host's settings as one ZIP file and restores such a file, on this PC or another one. The tray offers the same: **Back up settings** saves a file without secrets to the signed-in user's `Documents\Rubylight` folder and shows where, and **Restore settings...** opens the console's panel.

A backup holds `sunshine.conf` (every setting, webhooks included), the app list, the covers, app aliases, each paired device's own settings (names, display modes, commands and the other per-device overrides) and the game settings files kept per device. A `backup.json` inside lists every file with its size and SHA-256.

**Include pairing identity and sign-in** is off by default. Only with it on does the file hold secrets: this PC's pairing certificate and key, the paired devices with their certificates and permissions, the console sign-in, API tokens and webhook secrets. Anyone with such a file can pose as this PC to the paired devices and sign in to the console, so the console asks again before it downloads one. Without secrets, a sign-in password or token kept in `sunshine.conf` is left out and each webhook's secret is emptied.

Restoring works in steps:

1. **Check.** The console uploads the file and the host checks it: a Rubylight settings backup of a version it reads, every file as the manifest lists it, only files a backup holds, secrets only when the backup says it has them, and settings, apps and device files it can read. Nothing changes yet.
2. **Review.** The panel lists what changes: the settings that differ, apps added, removed and changed, covers and game settings files written, devices restored or skipped, and whether the pairing identity and sign-in stay or are replaced.
3. **Restore.** Refused while a device streams or connects. The host saves the current settings, with their secrets, as `backups\before-restore-<time>.zip` in its data folder (the newest five stay), then restarts and applies the backup before it loads anything else. A failed restore keeps the previous settings and shows a notice in the console; the file stays as `backups\restore-failed.zip`.

A backup without secrets keeps this PC's pairing identity, paired devices, permissions, sign-in and API tokens: device settings go only to the devices paired here (matched by their ID), and a webhook this PC does not have gets a new secret, which its receiver then needs. A backup with secrets replaces them, so a device paired since the backup was made must pair again. Covers and game settings files only this PC has stay. The API is in [the API reference](api.md#console-only-routes).

## All settings

Every setting in the console, by the console's tabs and groups, with its key and the default an unset key uses. Rows follow the console's order within each tab, with the collapsed troubleshooting groups last. Settings on the **Settings** home are in bold. Keys without a console field are under [Settings outside the console](#settings-outside-the-console). Search the console for a key to find its field.

### This PC

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| **Name and language: PC name** | `sunshine_name` | empty | The name your devices show for this PC. Empty uses its Windows name; local network discovery picks up a change after a restart. |
| Name and language: Fallback console language | `locale` | `en` (English) | Language used by the fallback console and reported to connected tools. This console is currently available in English only. |
| Updates: Check for updates | `update_check_interval` | `86400` s | How often Rubylight looks for a new release. Set it to 0 to turn off automatic checks and automatic installation. |
| Updates: Update check time | `update_check_time` | empty | Look for a new release once a day at this time on this PC's clock, in place of the interval. Leave it empty unless checks, and automatic installs, should happen at a set time. |
| **Updates: Install updates automatically** | `auto_update` | `false` | Download and install new releases on their own, only while nothing is streaming or running. Off, the console tells you about an update and you install it when you choose. |
| Updates: Include pre-releases | `notify_pre_releases` | `false` | Also offer pre-release versions when checking for updates. They get fixes sooner but may have more bugs. |
| Sleep and wake: Sleep after idle | `sleep_after_idle_minutes` | `0` min | Put this PC to sleep when nobody has streamed or used it for this many minutes. 0 turns it off. |
| Sleep and wake: Sleep with a suspended stream | `sleep_while_suspended` | `false` | Also let the PC sleep while a stream is suspended or an app launched from a device still runs. Off, either one keeps the PC awake. |
| Privacy: Lock Windows when the last device leaves | `lock_on_last_disconnect` | `false` | Lock this PC 10 seconds after the last device leaves, so it is not left signed in. Unlock it with your Windows password or PIN, from the next stream or at the PC. |
| Tray: Tray icon | `system_tray` | `true` | Show a Rubylight icon in the Windows notification area, with a shortcut to this console. |
| Tray: Hide tray controls | `hide_tray_controls` | `false` | Remove the disconnect, restart and quit actions from the tray icon's menu, so people at the PC cannot stop Rubylight from there. |
| Hardware metrics: Send hardware metrics to client | `hardware_metrics` | `true` | Lets a compatible device show this PC's CPU and GPU readings (load, temperatures, power) in its overlay during a stream. Leave it on: readings are taken only while a device asks. |
| Hardware metrics: Hardware metrics interval | `hardware_metrics_interval` | `1000` ms | The shortest time between two readings a streaming device asks for, from 500 to 10000 milliseconds. |
| Hardware metrics: Hardware metrics selection | `hardware_metrics_selection` | none | Which readings to send. Leave it empty to send every supported reading. |

### Picture

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| **Preset: Stream preset** | `stream_preset` | `balanced` (Balanced) | One choice that fills in the AMD and NVIDIA encoder settings you have not set yourself. Balanced suits almost everyone; settings you change below always win. |
| Streamed display: Display | `output_name` | empty | The monitor to stream when no virtual display is used. Empty streams the primary display; a device or app can choose its own. For testing (advanced, config file only): `window:<hwnd>/<pid>@<display>` streams that one window with WGC (the release harness's demo stream); `<pid>` is the process that owns the window and `<display>` the display the window is on (such as `\\.\DISPLAY1`). A malformed target, a closed or minimised window, a handle another process now owns, or a window on another display fails the stream with the reason, never falling back to a display. |
| **Streamed display: Virtual display** | `virtual_display_mode` | empty (Default (by Windows version)) | Streams use a virtual display, an extra screen at the device’s resolution and refresh rate, instead of a monitor. Default suits almost everyone; a device’s or app’s choice wins. |
| **Streamed display: Virtual display layout** | `virtual_display_layout` | `exclusive` (Exclusive) | What your monitors do while a stream uses a virtual display. Exclusive turns them off until the stream ends, so games open on the stream; an Extend layout keeps them on. |
| Streamed display: Virtual display scale | `dd_virtual_display_scale` | `0` (Keep) | The Windows scale (text and app size) of a virtual display; a fixed percentage Windows does not offer at that resolution stops the stream from starting. Keep leaves it to Windows. |
| Streamed display: Activate a virtual display | `dd_activate_virtual_display` | `false` | Every stream gets a virtual display, also where Virtual display is Off for the host, a device or an app. Leave it off; Virtual display is the usual way to choose. |
| Streamed display: Monitor setup | `dd_configuration_option` | `verify_only` (Only check that it’s on) | When a stream shows a monitor, not a virtual display, it can turn that monitor on, make it primary, or turn your others off until it ends. The default only checks that it is on. |
| Streamed display: Rotate the display | `dd_rotation_option` | `disabled` (Leave as it is) | Match the device gives the streamed display the device's portrait or landscape shape, also as it turns, for portrait streams. A streamed monitor turns too, until the stream ends. |
| Streamed display: Rotation | `dd_manual_rotation` | `0` (0°) | The angle the streamed display turns to, clockwise; a virtual display is made in that shape. A streamed monitor shows its picture turned by it until the stream ends. |
| Resolution and refresh rate: Resolution | `dd_resolution_option` | `auto` (Match the device) | The resolution of the streamed display. Match the device suits almost everyone: a streamed monitor switches to the device’s resolution, where it has that mode, and back afterwards. |
| Resolution and refresh rate: Fixed resolution | `dd_manual_resolution` | empty | WIDTHxHEIGHT, for example 2560x1440. The width can be 320 to 7680 and the height 200 to 4320. |
| Resolution and refresh rate: Refresh rate | `dd_refresh_rate_option` | `auto` (Match the stream) | The refresh rate of the streamed display. Match the stream suits almost everyone: a streamed monitor switches to the stream’s FPS, where it has that rate, and back afterwards. |
| Resolution and refresh rate: Fixed refresh rate | `dd_manual_refresh_rate` | empty | Streams fail to start while this is empty. In Hz, such as 59.94; a rate below the stream’s FPS cannot supply every frame, and a virtual display loses its faster automatic refresh. |
| Resolution and refresh rate: Default stream mode | `fallback_mode` | `1920x1080x60` | The resolution and frame rate used when a launch request names none, as WIDTHxHEIGHTxFPS. Devices normally send their own. |
| HDR: Prefer 10-bit SDR | `prefer_sdr_10bit` | `false` | When a device asks for HDR, sends 10-bit SDR instead and keeps the display in SDR: less banding, but no HDR picture. Leave it off if you want HDR. |
| HDR: HDR dynamic metadata | `hdr_dynamic_metadata` | `true` | Adds HDR10+ brightness statistics to HDR video, so a compatible display tone-maps scene by scene. Leave it on; turn it off if a display handles brightness worse with it. |
| HDR: Dolby Vision | `hdr_dolby_vision` | `true` | Offers Dolby Vision to devices that ask for it: profile 8.1 (8.4 while HLG is on) on HEVC HDR streams and profile 10.1 on AV1 HDR streams. Leave it on; others get HDR10 or HLG. |
| HDR: HLG | `hdr_hlg` | `true` | Offers HLG, an HDR format, to devices that ask for it on HEVC, AV1 and PyroWave HDR streams. Leave it on; devices that do not ask get HDR10 as before. |
| HDR: Display HDR | `dd_hdr_option` | `auto` (Match the stream) | Match the stream turns HDR on the streamed display on for an HDR stream, off for an SDR one, and back afterwards. Don’t change HDR can leave an HDR stream without HDR highlights. |
| HDR: HDR request | `dd_hdr_request_override` | `auto` (Follow the device) | Forces HDR on the streamed display on or off, for devices or games that report it wrongly; the stream itself still follows the device. Leave it on Follow the device otherwise. |
| HDR: Vulkan HDR on virtual displays | `vulkan_hdr_layer` | `true` | Lets Vulkan games use HDR on a virtual display, by registering a Vulkan layer (an add-on Vulkan games load) on this PC. Leave it on; turn it off only if Vulkan games crash. |
| HDR: Windows HDR Calibration on virtual displays | `virtual_display_hdr_calibration` | `false` | Gives an HDR virtual display the profile Windows HDR Calibration made for your HDR monitor. Leave it off: that profile describes your monitor, not the device you stream to. |
| HDR: Peak brightness | `hdr_peak_brightness` | `1000` nits | The HDR peak brightness, in nits, that a virtual display reports as its own. Set it to the brightest the device’s screen can show. |
| RTX HDR: RTX HDR | `rtx_hdr` | `false` | Turns a game’s SDR picture into HDR on NVIDIA graphics cards (NVIDIA build only). It runs only when an app or device turns it on and the device asks for HDR. |
| RTX HDR: SDR brightness | `rtx_hdr_sdr_brightness` | `0` | Raises the brightness of the SDR picture in an RTX HDR stream, from 0 to 100. |
| RTX HDR: Contrast | `rtx_hdr_contrast` | `0` | From −100 to 100; a value in the game’s NVIDIA driver profile wins. |
| RTX HDR: Saturation | `rtx_hdr_saturation` | `0` | From −100 to 100; a value in the game’s NVIDIA driver profile wins. |
| RTX HDR: Middle gray | `rtx_hdr_middle_gray` | `50` | Mid-tone brightness from 10 to 100; a value in the game’s NVIDIA driver profile wins. |
| Codecs: Full chroma | `experimental_packed_444` | `true` | Lets Nightjar devices ask for 4:4:4 (full-resolution color, for sharper colored text) over HEVC or AV1. Leave it on: a stream uses it only when the device’s own option asks. |
| Codecs: HEVC support | `hevc_mode` | `0` (Automatic) | Whether devices may use HEVC, and HDR over it. Leave it on Automatic, which offers what the encoder passed when Rubylight started; Off removes HEVC and SDR only removes its HDR. |
| Codecs: AV1 support | `av1_mode` | `0` (Automatic) | Whether devices may use AV1, and HDR over it. Leave it on Automatic, which offers what the encoder passed when Rubylight started; Off removes AV1 and SDR only removes its HDR. |
| Codecs: PyroWave | `pyrowave` | `true` | Lets compatible devices ask for PyroWave, a very low-latency GPU codec that needs hundreds of Mbps and a fast wired network. Leave it on: other devices never see it. |
| Codecs: Overlap PyroWave frames | `pyrowave_encode_overlap` | `false` | Prepares the next PyroWave frame while the GPU still encodes the current one: two frames are on the GPU at once, each with its own copy of the picture. Off encodes one frame at a time, as earlier versions did; at 4K and 120 fps the overlap measured no lower latency, so it is off by default. Applies to the next stream. |
| RTX Video enhancement: RTX Video enhancement | `rtx_video` | `false` | Cleans up and sharpens the picture on NVIDIA graphics cards before it is encoded, at the stream’s own size (NVIDIA build only). |
| RTX Video enhancement: Strength | `rtx_video_intensity` | `100` | How strongly RTX Video enhancement works, from 0 to 100. 0 leaves the picture unchanged. |
| Bitrate and network: Maximum bitrate | `max_bitrate` | `0` Kbps | Caps the bitrate a device can request, in Kbps (50000 is 50 Mbps). Leave it at 0, which uses the device's bitrate, unless your network or data plan needs a limit. |
| Bitrate and network: Error correction | `fec_percentage` | `20` % | Error correction (FEC): extra data that repairs lost packets. More survives a worse network but leaves less bitrate for the picture; leave it at 20% unless the picture breaks up. |
| Bitrate and network: Adapt bitrate on this PC | `host_adaptive_bitrate` | `false` | Lowers the bitrate when a Moonlight device reports video loss, then slowly raises it again. Leave it off: it takes over from the other automatic bitrate and pacing controls here. |
| Bitrate and network: Adapt error correction | `adaptive_fec` | `true` | Uses less error correction (FEC) while the device reports a clean link, which frees bitrate for the picture, and returns to the percentage above when it reports loss. Leave it on. |
| Bitrate and network: Most error correction a device may ask for | `fec_max_percentage` | `50` % | The most error correction (FEC) a device such as Nightjar may choose for itself, as a percentage of the video packets. Leave it at 50%; a lower cap limits the loss it can repair. |
| Bitrate and network: PyroWave error correction | `pyrowave_critical_fec_percentage` | `20` % | Error correction (FEC: extra data that repairs lost packets) for PyroWave pictures. More leaves less bitrate for the picture; leave it at 20% unless PyroWave breaks up. |
| Bitrate and network: Send batch size | `video_max_batch_size_kb` | `64` (64 KiB) | Largest burst of video packets sent at once. Leave it at 64 KiB unless a device drops bursts; smaller batches can help it, and need more send calls on this PC. |
| Bitrate and network: Pacing rate | `pacing_max_bitrate_kbps` | `0` Kbps | A fixed limit, in Kbps, on how fast video packets are sent; the bitrate stays. Leave it at 0 (automatic) unless a known slow link drops bursts: a value turns off adaptive pacing. |
| Bitrate and network: Adaptive packet pacing | `host_adaptive_pacing` | `false` | Sends video packets slowly at first, then faster, and backs off when the device reports loss. It adds delay when a stream starts; turn it on only for a link that loses bursts. |
| Bitrate and network: Adaptive packet pacing for Wi-Fi clients | `wireless_adaptive_pacing` | `true` | Paces video adaptively from the first frame, not after the first lost frames, for a client that reports Wi-Fi or cellular. Leave it on: streams lost fewer frames on weak Wi-Fi. |
| Bitrate and network: Slow down after video loss | `keyframe_loss_pacing` | `true` | After a lost keyframe (a full picture the stream restarts from) or repeated lost frames, spaces packets more evenly for the rest of the stream; the bitrate stays. Leave it on. |
| Bitrate and network: Adapt to network congestion | `congestion_control` | `false` | For devices that report packet arrival times, such as Nightjar: lowers the bitrate and paces video at the network's rate as delay builds up, before packets are lost, then raises it again. The lower of this and Lower the bitrate when the link cannot carry it applies. Off while Adapt bitrate on this PC is on or the device sets the bitrate. Reconnect after saving. |
| Frame pacing: Frame pacing | `frame_pacing` | `arrival` (On arrival) | On arrival encodes each new frame as soon as it is captured; leave it there. Fixed grid encodes the newest frame at fixed intervals, so frames wait for their slot, adding delay. |
| Frame pacing: Steady grid timing | `wgc_pacing_smoothing` | `true` | Keeps the fixed grid on a steady schedule. Leave it on: off starts each interval from the previous encode, so one late frame delays the ones after it. |
| Frame pacing: Still-picture resend rate | `minimum_fps_target` | `20` fps | How often an unchanged picture is sent again, not a limit on a game’s frame rate. Lower values save bandwidth on a still desktop; leave it at 20 fps unless you need that. |
| Frame pacing: Use the launch frame rate | `limit_framerate` | `false` | Encodes at the frame rate the device launched the app with, not the faster one its stream may request. Leave it off: some clients launch at 60, then ask for 120 or 240. |
| Frame pacing: Follow client size and frame rate changes | `stream_reconfigure` | `true` | Lets a Rubylight client change the stream resolution and frame rate without reconnecting, such as when a foldable phone unfolds. Leave it on; clients that never ask see no change. |
| Frame pacing: Hold capped games for the device's refresh | `phase_lock_hold_at_stream_rate` | `false` | With a Rubylight client's phase lock, holds each frame of a game running at the stream rate or slower until just before the device's refresh. Off, such frames are encoded as they arrive: a frame can't be sent earlier than the game makes it, only later. The hold cost 8 to 11 ms of latency on a Shield TV and halved its rare display gaps. |
| Frame pacing: Let devices use Frame Warp | `frame_warp` | `true` | Lets a Nightjar device that turns Frame Warp on show camera turns on its next screen refresh, without waiting for the round trip. The host learns how far each game turns per mouse or stick input and which parts of the picture stay put, such as the HUD, only for a device that asks. See [Frame Warp](frame-warp.md). |
| Frame limiter: Limit every stream | `frame_limiter_enable` | `false` | Caps games at the stream’s FPS on monitor streams too, for even frame pacing; the game then runs no faster than that on your monitor. Virtual-display streams are capped already. |
| Frame limiter: Limiter | `frame_limiter_provider` | `auto` (Automatic) | What caps the game’s FPS. Automatic and RTSS both use RivaTuner Statistics Server (RTSS), which must be installed; leave it on Automatic, or choose None to turn all limiting off. |
| Frame limiter: Frame limit | `frame_limiter_fps_limit` | `0` fps | The FPS games are capped at while streaming; 0 uses the stream’s frame rate. Leave it at 0 unless games should run at another rate; RTSS keeps decimals such as 59.94. |
| Frame limiter: Virtual display refresh | `frame_limiter_auto_virtual_framegen` | `legacy` (Twice the stream rate) | A virtual display’s refresh rate: a faster one captures each game frame sooner after it is drawn. Twice the stream rate suits almost everyone; Off also ends the automatic FPS cap. |
| Encoder: Encoder | `encoder` | `auto` (Automatic) | Leave it on Automatic: an encoder the graphics card lacks makes every stream fail. Automatic uses the card's own, AMD AMF or NVIDIA NVENC, and PyroWave's own for PyroWave streams. |
| Encoder: Graphics card | `adapter_name` | empty | The GPU that captures and encodes the video. Leave it on Automatic, which uses the card connected to the streamed display, unless this PC has several GPUs and you want another. |
| Encoder: Graphics card device ID | `adapter_pnp_id` | empty | Chooses the GPU by its Windows device instance ID instead of its name. Leave it empty unless this PC has two identical GPUs; when set, it takes precedence over the GPU name. |
| Encoder: Encoder focus | `encoder_focus` | `off` (Off) | Gives more bits to parts of the picture that stay still, such as text and HUDs, and fewer to motion. See [Encoder focus](#encoder-focus). |
| Capture: Capture method | `capture` | `auto` (Automatic) | Automatic uses direct driver push on Rubylight virtual displays and Windows Graphics Capture on other displays. AMD Direct Capture (`amd`) reads the Radeon driver's presented picture and falls back to Windows Graphics Capture; frame generation's extra frames are not streamed by any method. |
| Capture: What to stream | `capture_window` | `display` (The display) | The display, or only the launched app’s window. Apps and devices can choose their own. See [The app's window in place of a display](#the-apps-window-in-place-of-a-display). |
| Capture: When the window closes | `capture_window_closed` | `display` (Show the display) | What the stream does once the app’s window has closed and no other appeared within 5 seconds. The stream ends anyway when the window’s process exits. |
| Capture: Window’s program | `capture_window_exe` | empty | Streams only windows of this program, such as game.exe, also when a launcher started it. Leave it empty (any of the app’s programs) unless the stream shows the wrong window. |
| Capture: Window title contains | `capture_window_title` | empty | Streams only windows whose title contains this text, in any case. Leave it empty, which takes any title, unless the stream picks the wrong window. |
| After a stream: Restore displays on disconnect | `dd_config_revert_on_disconnect` | `true` | Puts your displays back as they were when a device disconnects, even while its game keeps running. Turn it off only to keep the stream’s display on the desktop for a reconnect. |
| After a stream: Restore delay | `dd_config_revert_delay` | `3000` ms | How long the stream’s display and layout are kept after a device disconnects, so a quick reconnect finds them in place. With an exclusive layout your monitors stay off that long. |
| After a stream: Keep a disconnected display for | `dd_paused_virtual_display_timeout_secs` | `7200` s | How long a disconnected device’s display is kept while its app runs; an exclusive layout's only with Keep monitors off while paused. 0 keeps it until the app closes. |
| After a stream: Keep monitors off while paused | `dd_hold_layout_while_paused` | `false` | With an exclusive layout, keep the monitors off while a device is disconnected, until it returns or its display is no longer kept: after the restore delay, or with Restore displays on disconnect off, after Keep a disconnected display for. |
| After a stream: Restore the saved layout | `dd_always_restore_from_golden` | `false` | After a stream, return the displays to the layout saved under **Advanced → Troubleshoot** instead of the layout from before the stream. |
| After a stream: Displays the saved layout skips | `dd_snapshot_exclude_devices` | none | A list of display device IDs in JSON format (text enclosed in square brackets) that restoring the saved layout leaves as they are, such as a dummy plug. |
| After a stream: Restore hotkey | `dd_snapshot_restore_hotkey` | empty | A key on this PC that ends every stream and restores the display layout, for when a stream leaves the screens unusable. F1 to F24, a letter or a digit. |
| After a stream: Restore hotkey modifiers | `dd_snapshot_restore_hotkey_modifiers` | `ctrl+alt+shift` | Keys held with the restore hotkey: ctrl, alt, shift and win, joined with +. Leave them unless another program uses the combination; none makes the key alone end every stream. |
| Remote monitors: Mute remote monitors | `remote_monitor_mute_audio` | `false` | A device used as a remote monitor (an extra screen for this PC) gets picture and input but no audio. Turn it on if such a device should stay silent. |
| Remote monitors: Remove when its stream ends | `remote_monitor_disconnect_on_stream_end` | `false` | Removes a device’s remote monitor from Windows when its stream ends. Leave it off so the device can resume where it was; turn it on if the leftover display gets in the way. |
| Remote monitors: Remove when the device disconnects | `remote_monitor_disconnect_on_client_disconnect` | `false` | Removes the remote monitor as soon as the device’s connection drops, even if it was kept for resuming. Leave it off unless a dropped device should not leave its display behind. |
| Capture troubleshooting: Capture poll interval | `capture_poll_interval_us` | `500` µs | For troubleshooting: how long capture waits before checking again when no new frame is ready. Shorter waits notice new frames sooner and use more CPU time; leave it at 500 µs. |
| Capture troubleshooting: GPU fence transport | `idd_push_fence_ring` | `true` | For troubleshooting: how frames from Rubylight’s virtual display driver reach the stream. Leave it on; turn it off only to rule it out when a virtual display’s picture misbehaves. |
| Capture troubleshooting: Predictive polling | `capture_predictive_poll` | `false` | For troubleshooting Desktop Duplication; leave it off. Checks for frames more often when the next one is due, which can catch them sooner in games with a steady frame rate. |
| Capture troubleshooting: Align capture with encoding | `wgc_slot_aligned_publish` | `false` | With Windows Graphics Capture, holds a new frame until just before the next encode if a newer one could arrive first. Try it if the display refreshes much faster than the stream. |
| Capture troubleshooting: Encode from GPU memory | `wgc_direct_encoder_input` | `true` | For troubleshooting an encoder. Leave it on: off copies every captured frame through system memory instead of handing it to the encoder on the GPU, which adds latency and CPU load. |
| Capture troubleshooting: Copy and convert on a compute queue | `gpu_compute_conversion` | `true` | For troubleshooting. Leave it on: on Radeon and NVIDIA graphics cards whose driver supports it, capture and color conversion run beside a game instead of waiting behind it. |
| AMD AMF: Usage | `amd_usage` | `ultralowlatency` (Ultra low latency) | AMD’s base encoding profile; the settings below override parts of it. Leave it on Ultra low latency: Low latency, High quality and Transcoding queue frames at 4K, which adds delay. |
| AMD AMF: Quality preset | `amd_quality` | `speed` (Speed) | Trades encoding speed for picture quality. Leave it on Speed, the lowest delay: Quality took about 1 ms longer per 4K HEVC frame and 12 ms per AV1 frame on an RX 7900 XT. |
| AMD AMF: Rate control | `amd_rc` | `vbr_latency` (Latency-constrained VBR) | How the encoder keeps to the bitrate. Leave it on Latency-constrained VBR: a mode the driver lacks fails the stream, quality modes add delay, Constant QP ignores the bitrate. |
| AMD AMF: Peak bitrate | `amd_peak_bitrate_ratio` | `0` (Driver default) | The highest bitrate the encoder may reach, as a multiple of the stream bitrate. Leave it on Driver default unless you tune rate control; it does not limit single frames. |
| AMD AMF: Rate-control buffer | `amd_vbv_buffer_frames` | `0` (Driver default) | Rate-control buffer (VBV), in frames at the stream bitrate. Smaller budgets can reduce bursts at the cost of picture quality; leave it on Driver default unless bursts cause loss. |
| AMD AMF: Maximum frame size | `amd_max_frame_size` | `auto` (Automatic — follow network pace) | Keeps each frame, recovery keyframes included, within what error correction protects. Leave it on Automatic: tight limits can cost quality; Driver default lets frames outgrow that. |
| AMD AMF: Quality VBR level | `amd_qvbr_quality_level` | `0` | Target quality for quality VBR, from 1 (lowest) to 51 (highest). 0 keeps the driver default. |
| AMD AMF: Pre-analysis | `amd_preanalysis` | `false` | Analyzes each frame before encoding to improve rate control. Leave it off: it added about 20 ms per 4K frame on 8-bit SDR streams (RX 7900 XT), and HDR streams never use it. |
| AMD AMF: Variance-based adaptive quantization | `amd_vbaq` | empty (Default (H.264 off; HEVC/AV1 on)) | VBAQ gives more bits to smooth areas, where artifacts are easiest to see, and fewer to busy textures. Leave it on Default: for H.264, On lowered picture quality in our comparison. |
| AMD AMF: Enforce decoder buffer limits | `amd_enforce_hrd` | `false` | Holds rate control to the decoder buffer model (HRD), greatly reducing bitrate spikes. It can cause artifacts or lower quality on some GPUs; leave it off unless spikes cause loss. |
| AMD AMF: H.264 entropy coding | `amd_coder` | `auto` (Driver default) | H.264 only. CABAC gives better quality per bit and CAVLC is simpler to decode; leave it on Driver default unless a device cannot keep up decoding H.264. |
| AMD AMF: Input queue size | `amd_input_queue_size` | `0` frames | How many frames may wait for the encoder. Leave it at 0, the driver default: forcing a queue of one exposed a driver freeze on RDNA4 cards, with video stalling while audio plays. |
| AMD AMF: Long-term reference frames | `amd_ltr_frames` | `4` frames | Long-term reference (LTR) frames: a device that lost packets recovers with a small frame instead of a full keyframe. Leave it at 4; use 0 only for a decoder that freezes on them. |
| AMD AMF: Low-latency mode | `amd_lowlatency_mode` | `auto` (Driver default) | Leave it on Driver default: forcing the encoder's low-latency mode froze HEVC video on RX 9000 cards (Adrenalin 26.5). The default Usage already has it on for H.264 and HEVC. |
| AMD AMF: High-motion quality boost | `amd_high_motion_quality_boost` | `auto` (Driver default) | Asks the driver to improve picture quality in fast motion, where it supports that. Leave it on Driver default unless you compare it on your own games. |
| AMD AMF: SmartAccess Video | `amd_smart_access_video` | `auto` (Driver default) | Lets the driver share encoding between AMD integrated and discrete graphics on systems with SmartAccess Video. Leave it on Driver default; set Off while diagnosing a freeze. |
| AMD AMF: Split-frame encoding | `amd_split_frame` | `auto` (Automatic) | Lets the driver split HEVC or AV1 frames across two encoder engines. Leave Automatic unless comparing your own GPU. |
| AMD AMF: Hardware color conversion | `amd_vpe` | `false` (Off) | Converts and scales frames with AMF's video converter instead of Rubylight's shaders, for YUV 4:2:0 streams; other frames keep the shaders. |
| AMD AMF: AV1 latency mode | `amd_av1_latency_mode` | `auto` (Driver default) | How quickly each AV1 frame is finished; lower latency uses more power. Leave it on Driver default: with the default Usage the driver already picks the lowest latency. |
| AMD AMF: AV1 screen content tools | `amd_av1_screen_content` | `auto` (Driver default) | AV1 only. Coding tools for desktop content, which can make text and interface edges clearer; leave it on Driver default unless you stream a desktop in AV1 and want to compare. |
| AMD AMF: AV1 tiles | `amd_av1_tiles` | `0` (Automatic) | How many tiles each AV1 frame is split into. Leave it on Automatic, which follows the device's slice request, up to 4; a count the driver refuses makes the stream fail. |
| NVIDIA NVENC: Preset | `nvenc_preset` | `1` (P1 (fastest)) | Higher presets compress better at the same bitrate but take longer to encode each frame. Leave it on P1, the fastest, unless picture quality matters more to you than latency. |
| NVIDIA NVENC: Rate control | `nvenc_rate_control` | `cbr` (Constant bitrate) | Constant bitrate (CBR) holds each frame to the stream bitrate; leave it there. Variable bitrate (VBR) spends less on simple scenes and lets busy ones peak higher, in bigger bursts. |
| NVIDIA NVENC: Target quality | `nvenc_target_quality` | `0` | The quality variable bitrate aims for: lower values look better and use more bits. Leave it at 0, which lets the driver choose, unless you tune VBR. |
| NVIDIA NVENC: Peak bitrate | `nvenc_peak_bitrate_ratio` | `1.5` (1.5× stream bitrate) | How far variable bitrate may go above the stream bitrate on busy scenes. Higher is sharper, in bigger bursts that a slow or lossy network can drop; leave it at 1.5× if unsure. |
| NVIDIA NVENC: Multi-pass encoding | `nvenc_twopass` | `quarter_res` (Quarter resolution) | A first pass over each frame spreads bits better and keeps frames within the bitrate. Leave it on Quarter resolution: Off can cause bitrate spikes and packet loss. |
| NVIDIA NVENC: Spatial adaptive quantization | `nvenc_spatial_aq` | `true` | Gives flat areas, where blocking shows most, more bits and detailed areas fewer. Adds no latency; leave it on. |
| NVIDIA NVENC: Temporal adaptive quantization | `nvenc_temporal_aq` | `false` | Spends more bits on parts of the picture that stay the same across frames. Leave it off unless you want to compare; GPUs without it encode without it and log a warning. |
| NVIDIA NVENC: Split-frame encoding | `nvenc_split_encode` | `auto` (Automatic) | Splits each HEVC or AV1 frame across several encoder engines on GPUs that have them. Leave it on Automatic, which lets the driver decide. |
| NVIDIA NVENC: Frame time guard | `nvenc_frame_budget_guard` | `true` | Encodes with a faster preset when the chosen one would not finish each frame in time at the stream resolution and frame rate. Leave it on so a slow preset cannot fall behind. |
| NVIDIA NVENC: Realtime GPU priority with hardware scheduling | `nvenc_realtime_hags` | `true` | Keeps Rubylight at realtime GPU priority while hardware-accelerated GPU scheduling (HAGS) is on, so NVENC keeps up with a game that fills the GPU. Leave it on. |
| NVIDIA NVENC: Full GPU clocks while streaming | `nvenc_latency_over_power` | `true` | Sets NVIDIA's power management to prefer maximum performance for Rubylight, so NVENC encodes at full clocks when a game or the desktop barely loads the GPU. Leave it on. |
| NVIDIA NVENC: Frame size headroom | `nvenc_vbv_increase` | `0` % | Lets a single frame exceed the average frame size by this much (a larger VBV). It helps busy scenes but can cause packet loss; leave it at 0 unless your network is fast and steady. |
| NVIDIA NVENC: Weighted prediction | `nvenc_weighted_prediction` | `false` | Improves quality during fades and brightness changes on GPUs that support it. Leave it off: it disables HEVC split-frame encoding and needs Copy and convert on a compute queue off. |
| NVIDIA NVENC: Use CAVLC for H.264 | `nvenc_h264_cavlc` | `false` | Uses the simpler CAVLC entropy coding instead of CABAC, which needs more bitrate for the same quality. |
| NVIDIA NVENC: Insert filler data | `nvenc_insert_filler_data` | `false` | Pads frames so the stream holds a constant bitrate even when the picture is simple. That uses more bandwidth for the same picture; leave it off unless you need a steady rate. |
| NVIDIA NVENC: Minimum quantizer | `nvenc_enable_min_qp` | `false` | Stops the encoder from spending bits beyond a set quality level on easy scenes, leaving headroom for hard ones. It caps the best picture quality; leave it off unless you tune this. |
| NVIDIA NVENC: Minimum QP for H.264 | `nvenc_min_qp_h264` | `19` | The lowest quantization parameter (QP), which controls compression, for H.264 frames. Higher values cap quality sooner; leave it at 19 unless you tune this. |
| NVIDIA NVENC: Minimum QP for HEVC | `nvenc_min_qp_hevc` | `23` | The lowest quantization parameter (QP), which controls compression, for HEVC frames. Higher values cap quality sooner; leave it at 23 unless you tune this. |
| NVIDIA NVENC: Minimum QP for AV1 | `nvenc_min_qp_av1` | `23` | The lowest quantization parameter (QP), which controls compression, for AV1 frames, from 0 to 255. Higher values cap quality sooner; leave it at 23 unless you tune this. |
| Mode substitutions: Mode substitutions (JSON) | `dd_mode_remapping` | `{"mixed": [], "resolution_only": [], "refresh_rate_only": []}` | Changes a requested mode before it is applied. Each entry matches requested_resolution and requested_fps and sets final_resolution and final_refresh_rate. |
| RTSS: RTSS folder | `rtss_install_path` | empty | Where RivaTuner Statistics Server (RTSS) is installed. Leave it empty, which looks in Program Files, unless RTSS is installed elsewhere and is not found. |
| RTSS: RTSS limit mode | `rtss_frame_limit_type` | `async` (Asynchronous) | How RTSS paces frames. Some streams use front edge sync instead. |
| RTSS: Use this mode on virtual displays | `rtss_allow_virtual_display_override` | `false` | Applies the RTSS limit mode above to virtual-display streams, in place of the mode they choose themselves. Leave it off unless you want to try another RTSS mode there. |

### Sound & controls

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| **Sound: Stream audio** | `stream_audio` | `true` | Send this PC's sound to the device. Turn it off when a stream is only used as an extra monitor and should stay silent. |
| Sound: Audio to stream | `audio_capture_mode` | `system` (System audio (selected playback device)) | System audio suits almost everyone. Game only streams the launched game alone, without browsers and notifications, but in stereo only and silent when the game cannot be verified. |
| Sound: PCM and Dolby Digital audio | `audio_passthrough` | `true` | Offers uncompressed PCM, Dolby Digital and Dolby Digital Plus to devices that ask for it, for TVs and receivers that decode it themselves; leave it on. Other devices keep Opus. |
| Sound: 7.1.4 surround | `audio_surround_714` | `true` | Offers 7.1.4 surround sound to devices that ask for it; leave it on, since no other device is affected. Height channels stay silent unless the captured playback device has them. |
| Audio devices: Audio device | `audio_sink` | empty | The audio output captured when the streaming device keeps sound playing on this PC, or when no virtual speakers exist. Leave it empty to use the Windows default output. |
| Audio devices: Virtual speakers | `virtual_sink` | empty | The audio output made the Windows default during a stream, so sound goes to the device instead of the PC's speakers. Leave it empty to use Steam Streaming Speakers when installed. |
| Audio devices: Capture without switching | `audio_sink_capture_only` | `false` | Capture the audio device without making it the Windows default output. Leave it off unless you want to send apps to that device yourself: only what plays there reaches the stream. |
| Audio devices: Follow the default device | `auto_capture_sink` | `true` | Capture whichever device is the Windows default, even if it changes during a stream. Leave it on; turn it off only to stay on the device chosen when the stream started. |
| Virtual speakers: Keep virtual speakers as default | `keep_sink_default` | `true` | If another device becomes the Windows default mid-stream, switch back to the virtual speakers so the stream keeps its sound. Leave it on; the device you picked returns afterwards. |
| Virtual speakers: Keep virtual speakers while paused | `hold_sink_while_paused` | `false` | Keeps this PC silent when a device disconnects and its game keeps running: the virtual speakers stay the Windows default until a device returns. Off, the PC plays the game's sound. |
| Virtual speakers: Install Steam audio drivers | `install_steam_audio_drivers` | `true` | When Steam is installed, install its streaming speakers and microphone when a stream needs them. Leave it on: they keep the PC silent and carry surround and a device's microphone. |
| **Microphone: Use the device microphone** | `stream_mic` | `true` | Let a streaming device talk through this PC with its own microphone, where the client supports it. Leave it on: a device sends its microphone only when you turn that on there. |
| Input devices: Keyboard | `keyboard` | `true` | Let devices type on this PC. |
| Input devices: Mouse | `mouse` | `true` | Let devices use the mouse on this PC. Pen and touch input need this too. |
| **Input devices: Controllers** | `controller` | `true` | Let devices use game controllers on this PC. |
| Input devices: Remote input entry | `enable_input_only_mode` | `false` | Add a Remote Input entry to every device's app list. It sends keyboard, mouse and controller input to this PC without video, for example from a second device. |
| Keyboard: Send scan codes | `always_send_scancodes` | `true` | Send keys as scan codes (the codes a real keyboard sends), which some games need to see keyboard input at all. Leave it on; turn it off only if a device types the wrong characters. |
| Keyboard: Right Alt as Windows key | `key_rightalt_to_key_win` | `false` | Treat the right Alt key as the Windows key, for devices that cannot send the Windows key. The right Alt key then no longer works as Alt or AltGr. |
| Keyboard: Key repeat delay | `key_repeat_delay` | `500` ms | How long a held key waits before it starts repeating. 0 turns key repeat off on this PC. |
| Keyboard: Key repeat rate | `key_repeat_frequency` | `24.9` Hz | How many times per second a held key repeats. Decimals are allowed; 0 uses the default. |
| Keyboard: Keyboard follows text fields | `keyboard_follows_focus` | `true` | Show the device's keyboard when a text field on this PC gets the focus, and hide it after, on devices that support it. Turn it off if the keyboard opens when you do not want it. |
| Keyboard: Send the text around the cursor | `text_input_surrounding_text` | `false` | Send the text around the cursor to the device's keyboard for its suggestions, never from password fields. Leave it off if text on this PC should not reach the device's keyboard. |
| Keyboard: Keyboard layout | `keyboard_layout` | `client` | Which layout turns the device's keys into scan codes. Leave it on client, the layout the device reports, unless keys type wrong characters; us is US English, or enter a layout id. |
| Mouse, pen and touch: High-resolution scrolling | `high_resolution_scrolling` | `true` | Pass fine scroll steps from the device. Turn it off for older apps that scroll too far, so scrolling moves in whole notches. |
| Mouse, pen and touch: Native pen and touch | `native_pen_touch` | `true` | Pass pen and touch from devices to Windows as pen and touch. Turn it off for apps that handle them badly; devices then send touch as mouse input. |
| Mouse, pen and touch: Mouse input method | `mouse_mode` | `auto` (Automatic) | How this PC moves its mouse for a device: auto (the virtual mouse when the USB audio and haptics driver is installed), virtual_mouse or injection. |
| Mouse, pen and touch: Touchpad gestures from devices | `native_touchpad` | `true` | Replay a device's physical touchpad on a virtual Windows precision touchpad, so Windows' scrolling and gestures work. |
| Mouse, pen and touch: Pen barrel roll | `pen_barrel_roll` | `true` | Passes on how far a stylus is turned around its own axis (barrel roll), for pens and devices that report it. Leave it on; it needs Native pen and touch. |
| Mouse, pen and touch: Touch | `touch_mode` | `client` (As the device sends them) | How this PC takes a device's touches: as sent, or as a mouse or trackpad for games that handle a mouse better than touch. Needs native pen and touch. |
| Mouse, pen and touch: Touch sensitivity | `touch_sensitivity` | `100` % | How far the pointer moves for a swipe as a trackpad, and how far two fingers scroll, in percent. Raise it if swipes move too little; lower it if the pointer overshoots. |
| Mouse, pen and touch: Touch gestures | `touch_gestures` | `true` | As a mouse or trackpad, two fingers scroll and a two-finger tap clicks right. Leave it on unless a second finger must not end a drag: as a mouse it lets go of the left button. |
| Mouse, pen and touch: Device-drawn cursor | `client_cursor` | `true` | Lets a device draw the mouse cursor itself, so it moves without waiting for the video. Leave it on; it needs Desktop Duplication capture, otherwise the cursor stays in the video. |
| Controllers: Controller type | `gamepad` | `auto` (Automatic) | The virtual controller this PC uses. Automatic matches each device's controller; choosing a type here uses it for all connected controllers. `usbip_switch2` is a Switch 2 Pro Controller over USB/IP. |
| Controllers: PlayStation controller for motion controls | `motion_as_ds4` | `true` | With Controller type on Automatic, a gyro makes the controller a DualSense, so games get motion controls. Turn it off for games that see only Xbox controllers, which have no gyro. |
| Controllers: PlayStation controller for touchpads | `touchpad_as_ds4` | `true` | With Controller type on Automatic, a touchpad makes the controller a DualSense, so games get the touchpad. Turn it off for games that see only Xbox controllers, which have none. |
| Controllers: Motion server for emulators (DSU) | `enable_dsu_server` | `false` | Emulators such as Cemu, Dolphin and Citra read each streamed controller's gyroscope, accelerometer, buttons and touch from this PC. Listens on every network interface. Takes effect after a host restart. |
| Controllers: Motion server port | `dsu_server_port` | `26760` | The UDP port emulators connect to, 1024 to 65535. Takes effect after a host restart. |
| Controllers: Vibration | `forward_rumble` | `true` | Send vibration from games back to the device's controller. |
| Controllers: Haptics from game audio | `audio_haptics` | `false` | A DualSense on the device feels the game's bass and impacts when the game sends it no vibration of its own. A matter of taste: turn it on to try it; changes apply during a stream. |
| Controllers: Audio haptics strength | `audio_haptics_strength` | `100` % | How strongly the bass drives the actuators. 0 is off; above 100 % quiet scenes are felt more but loud ones clip. |
| Controllers: Audio haptics response | `audio_haptics_curve` | `linear` (Linear) | Linear follows the sound; Soft lifts quiet bass, and Punchy keeps only the big hits. |
| Controllers: Audio haptics noise gate | `audio_haptics_gate` | `-50` dBFS | Bass quieter than this is not felt, so music and ambience under it stay still. Raise it if the controller hums. |
| Controllers: Hold Back for Guide | `back_button_timeout` | `-1` ms | Holding Back (Select) this long presses the Guide (Home) button instead, for controllers without one. -1 turns this off. |
| Steam Deck and back grips: Steam Deck controller | `steam_deck_controller` | `auto` (Automatic) | How a Steam Deck's controls reach this PC. Automatic suits almost everyone: with Steam running here it is a real Steam Deck with its Steam Input layout, else a virtual controller. |
| Steam Deck and back grips: Back grip L4 | `back_grip_l4` | `none` (Nothing) | What the upper left back button presses: a Steam Deck's L4, or the matching paddle on an Xbox Elite or DualSense Edge. |
| Steam Deck and back grips: Back grip R4 | `back_grip_r4` | `none` (Nothing) | What the upper right back button presses: a Steam Deck's R4, or the matching paddle on an Xbox Elite or DualSense Edge. |
| Steam Deck and back grips: Back grip L5 | `back_grip_l5` | `none` (Nothing) | What the lower left back button presses: a Steam Deck's L5, or the matching paddle on an Xbox Elite or DualSense Edge. |
| Steam Deck and back grips: Back grip R5 | `back_grip_r5` | `none` (Nothing) | What the lower right back button presses: a Steam Deck's R5, or the matching paddle on an Xbox Elite or DualSense Edge. |
| USB devices: USB forwarding | `usb_forwarding` | `false` | Let the devices you allow plug their USB devices into this PC during a stream. Each device also needs the USB devices permission, and this PC the usbip-win2 driver. |
| USB devices: USB devices at once | `usb_max_devices` | `4` | How many USB devices this PC takes from devices at the same time. |
| Key remapping: Key remapping (JSON) | `keybindings` | none | Makes a key from the device press a different key on this PC, as pairs of Windows virtual-key codes such as ["0x10", "0xA0"]. |

### Games

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| App behavior: Quit the game on the first request | `remote_monitor_terminate_on_first_request` | `false` | Lets another device quit the running game with its first request. Leave it off: it must then ask again within 60 seconds, and the device that started the game is never asked twice. |
| App behavior: Confirm before replacing a running app | `remote_monitor_confirm_app_replacement` | `true` | A device must ask twice to replace a running app with a different one, so a game is not closed by accident. Leave it on; turned off, the first request closes the running app. |
| **App behavior: Close the app when the last device disconnects** | `terminate_on_pause` | `false` | Closes the running app when its last streaming device disconnects; unsaved progress may be lost. Leave it off to keep the game running so you can reconnect and carry on. |
| App behavior: Exit timeout | `exit_timeout` | `10` s | How long an app gets to close by itself when Rubylight ends it, before it is ended by force; 0 ends it at once. Raise it if a game needs longer to shut down cleanly. |
| Steam options: Steam library | `steam_enabled` | `true` | Adds your Steam games to the library, with their covers, and ends a stream when the game it started exits. Off adds no more games; added ones stay. |
| Steam options: Sync automatically | `steam_auto_sync` | `true` | Checks Steam every 30 seconds and updates the Steam apps when games are installed, removed or played. Off, the list changes only when you press Sync now on the Games page. |
| Steam options: Every installed game | `steam_sync_all_installed` | `true` | Adds every installed game, as every launcher does. Turn it off for a shorter list: only the most recently played games are added, and others the sync added are removed. |
| Steam options: Recent games | `steam_recent_games` | `10` | How many of the most recently played games to add. |
| Steam options: Played within | `steam_recent_max_age_days` | `30` days | Leaves out games not played for this many days. 0 adds them however long ago they were played. |
| Steam options: Remove uninstalled games | `steam_autosync_remove_uninstalled` | `true` | Removes a Steam app when its game is uninstalled; off, it stays in the list. With recent games only, apps that fall out of the list are always removed. |
| Steam options: Include tools | `steam_include_tools` | `false` | Also adds what Steam lists as installed besides games: tools, runtimes, redistributables and DLC. Leave it off unless you want to start one of them from a device. |
| Steam options: Games to leave out | `steam_exclude_games` | none | A JSON list of games the sync never adds, by Steam app ID or name: ["570", {"name": "Dota 2"}]. Their Steam apps are removed. |
| Epic Games options: Epic Games library | `library_epic_enabled` | `true` | Adds the games installed with the Epic Games Launcher to the library, with their covers, and ends a stream when the game exits. Off adds no more games; added ones stay. |
| Epic Games options: Sync automatically | `epic_auto_sync` | `true` | Checks every five minutes for games installed or removed, and updates the library. When off, Sync now in Games still adds them. |
| Epic Games options: Remove uninstalled games | `epic_autosync_remove_uninstalled` | `true` | Removes a game’s app when the game is uninstalled; off, it stays in the list. Games you added by hand always stay. |
| Epic Games options: Games to leave out | `epic_exclude_games` | none | A JSON list of games the sync never adds, by name or Epic Games ID: [{"name": "Fortnite"}]. Apps already added for them are removed. |
| Battle.net options: Battle.net library | `library_battlenet_enabled` | `true` | Adds the games installed with Battle.net to the library, and ends a stream when the game exits. Off adds no more games; added ones stay. |
| Battle.net options: Sync automatically | `battlenet_auto_sync` | `true` | Checks every five minutes for games installed, removed or played, and updates the library. When off, Sync now in Games still adds them. |
| Battle.net options: Every installed game | `battlenet_sync_all_installed` | `true` | Adds every installed game, as every launcher does. Turn it off for a shorter list: only the most recently played games are added, and others the sync added are removed. |
| Battle.net options: Recent games | `battlenet_recent_games` | `10` | How many of the most recently played games to add. |
| Battle.net options: Played within | `battlenet_recent_max_age_days` | `30` days | Leaves out games not played for this many days. 0 adds them however long ago they were played. |
| Battle.net options: Remove uninstalled games | `battlenet_autosync_remove_uninstalled` | `true` | Removes a game’s app when the game is uninstalled; off, it stays in the list. With recent games only, apps that fall out of the list are always removed. |
| Battle.net options: Games to leave out | `battlenet_exclude_games` | none | A JSON list of games the sync never adds, by name or Battle.net ID: [{"name": "Overwatch 2"}]. Apps already added for them are removed. |
| Microsoft Store and Xbox options: Microsoft Store and Xbox library | `library_microsoft_enabled` | `true` | Adds the games installed with the Xbox app or the Microsoft Store to the library, with their icons, and ends a stream when the game exits. Off adds no more games; added ones stay. |
| Microsoft Store and Xbox options: Sync automatically | `microsoft_auto_sync` | `true` | Checks every five minutes for games installed or removed, and updates the library. When off, Sync now in Games still adds them. |
| Microsoft Store and Xbox options: Remove uninstalled games | `microsoft_autosync_remove_uninstalled` | `true` | Removes a game’s app when the game is uninstalled; off, it stays in the list. Games you added by hand always stay. |
| Microsoft Store and Xbox options: Games to leave out | `microsoft_exclude_games` | none | A JSON list of games the sync never adds, by name or Microsoft Store and Xbox ID: [{"name": "Halo Infinite"}]. Apps already added for them are removed. |
| EA app options: EA app library | `library_ea_enabled` | `true` | Adds the games installed with the EA app to the library, and ends a stream when the game exits. Off adds no more games; added ones stay. |
| EA app options: Sync automatically | `ea_auto_sync` | `true` | Checks every five minutes for games installed or removed, and updates the library. When off, Sync now in Games still adds them. |
| EA app options: Remove uninstalled games | `ea_autosync_remove_uninstalled` | `true` | Removes a game’s app when the game is uninstalled; off, it stays in the list. Games you added by hand always stay. |
| EA app options: Games to leave out | `ea_exclude_games` | none | A JSON list of games the sync never adds, by name or EA app ID: [{"name": "Apex Legends"}]. Apps already added for them are removed. |
| Ubisoft Connect options: Ubisoft Connect library | `library_ubisoft_enabled` | `true` | Adds the games installed with Ubisoft Connect to the library, and ends a stream when the game exits. Off adds no more games; added ones stay. |
| Ubisoft Connect options: Sync automatically | `ubisoft_auto_sync` | `true` | Checks every five minutes for games installed or removed, and updates the library. When off, Sync now in Games still adds them. |
| Ubisoft Connect options: Remove uninstalled games | `ubisoft_autosync_remove_uninstalled` | `true` | Removes a game’s app when the game is uninstalled; off, it stays in the list. Games you added by hand always stay. |
| Ubisoft Connect options: Games to leave out | `ubisoft_exclude_games` | none | A JSON list of games the sync never adds, by name or Ubisoft Connect ID: [{"name": "Assassin's Creed Mirage"}]. Apps already added for them are removed. |
| GOG options: GOG library | `library_gog_enabled` | `true` | Adds the games installed with GOG Galaxy or the GOG installer to the library, with their icons, and ends a stream when the game exits. Off adds no more games; added ones stay. |
| GOG options: Sync automatically | `gog_auto_sync` | `true` | Checks every five minutes for games installed or removed, and updates the library. When off, Sync now in Games still adds them. |
| GOG options: Remove uninstalled games | `gog_autosync_remove_uninstalled` | `true` | Removes a game’s app when the game is uninstalled; off, it stays in the list. Games you added by hand always stay. |
| GOG options: Games to leave out | `gog_exclude_games` | none | A JSON list of games the sync never adds, by name or GOG ID: [{"name": "Cyberpunk 2077"}]. Apps already added for them are removed. |
| Playnite: Playnite | `playnite_enabled` | `true` | Adds your Playnite games, starts them through Playnite and ends the stream when the game closes. Needs the Rubylight plugin in Playnite; off, the games it added no longer start. |
| Playnite: Add games automatically | `playnite_auto_sync` | `true` | While Playnite runs, adds the games chosen below and removes them when they no longer qualify. Off, no game is added or removed; Sync now only updates those already there. |
| Playnite: Every installed game | `playnite_sync_all_installed` | `true` | Adds every installed game in Playnite, as every launcher does. Turn it off for a shorter list: only recently played games and the categories and libraries below are added. |
| Playnite: Recent games | `playnite_recent_games` | `10` | How many of the most recently played games to add. 0 adds none. |
| Playnite: Played within | `playnite_recent_max_age_days` | `30` days | Recent games must have been played within this many days. 0 counts any game, played or not. |
| Playnite: Categories to add | `playnite_sync_categories` | none | Also adds the installed games in these Playnite categories, as a JSON list of names: ["Couch", "Racing"]. Has an effect only while Every installed game is off. |
| Playnite: Libraries to add | `playnite_sync_plugins` | none | Also adds the installed games that come from these Playnite library plugins, as a JSON list of plugin IDs. Has an effect only while Every installed game is off. |
| Playnite: Games to leave out | `playnite_exclude_games` | none | A JSON list of Playnite game IDs the sync never adds. Deleting a game on the Games page keeps it out too, with no list to write. |
| Playnite: Categories to leave out | `playnite_exclude_categories` | none | A JSON list of category names whose games the sync never adds. |
| Playnite: Libraries to leave out | `playnite_exclude_plugins` | none | A JSON list of library plugin IDs whose games the sync never adds. |
| Playnite: Remove uninstalled games | `playnite_autosync_remove_uninstalled` | `true` | Removes an added app when its game is uninstalled. |
| Playnite: Remove unplayed after | `playnite_autosync_delete_after_days` | `14` days | Removes an added app not played within this many days of being added. 0 keeps it. |
| Playnite: Replace older recent games | `playnite_autosync_require_replacement` | `true` | When newer recent games are added, removes as many older added games, so the list keeps its size. Off, the older ones stay and the list grows. |
| Playnite: Playnite fullscreen app | `playnite_fullscreen_entry_enabled` | `false` | Adds a “Playnite (Fullscreen)” app that opens Playnite’s fullscreen mode, for choosing a game on the device. |
| Playnite: Wait after a game closes | `playnite_exit_wait_secs` | `15` s | After a game closes, the stream shows the desktop this long, then ends. Lower it to leave sooner; too low can end it while a game restarts itself or starts through a launcher. |
| Playnite launch focus: Confirm game focus | `playnite_focus_attempts` | `3` | Puts a launched game or Playnite menu in front, so input reaches it, until confirmed there this many times. Lower it if that interrupts you; a launcher may then take focus back. |
| Playnite launch focus: Stop focusing after | `playnite_focus_timeout_secs` | `15` s | The longest Rubylight keeps bringing a launched game or Playnite menu to the front. Raise it if a slow launch leaves the game behind another window; 0 leaves focus alone. |
| Playnite launch focus: Stop after the first focus confirmation | `playnite_focus_exit_on_first` | `false` | Stops focusing the game or Playnite menu once it was confirmed in front. Turn it on if launches keep interrupting your window; a launcher or splash screen may then take focus back. |
| Game folders: Game folders | `library_local_folders` | none | Folders that hold one game per subfolder, such as D:\Games, for games no launcher lists. Each subfolder joins the library with the program that starts it. |
| Game folders: Sync automatically | `library_local_auto_sync` | `true` | Checks the game folders every five minutes and updates their games when they are added or removed. When off, Sync now in Games still adds them. |
| Game folders: Remove deleted games | `library_local_autosync_remove_uninstalled` | `true` | Removes a game folder’s app when its folder is deleted; off, it stays in the list. Games you added by hand always stay. |
| Game folders: Folders to leave out | `library_local_exclude` | none | Subfolders to skip in your game folders, by name (Tools) or full path (D:\Games\Tools). They are never added, and Add games never lists them. |
| Play together: Nucleus Co-op folder | `nucleus_path` | empty | The folder you unpacked Nucleus Co-op 2.4.2 into, under C:\Program Files; co-op and background streaming need it. Empty uses an earlier Rubylight's install, if any. |
| Play together: Co-op players | `nucleus_coop_players` | `4` | The most players a Nucleus co-op session gets, 1 to 4; each player runs their own instance of the game on this PC. The game's handler and the free controller slots can allow fewer. |
| Play together: When another device joins the running game | `guest_join` | `off` (Off, stream beside the game) | Whether a paired device that resumes the game another device plays joins as a guest, and whether the player lets it in first. Off streams it beside the game, as before guests. |
| Play together: What a guest may do | `guest_permission` | `controller` (Controller) | What a guest may do unless the player chooses otherwise for that guest. Controller suits playing together; Controller, keyboard and mouse also lets a guest operate this PC. |
| Lossless Scaling: Lossless Scaling program | `lossless_scaling_path` | empty | LosslessScaling.exe or its folder, for apps that use Lossless Scaling. Leave it empty unless it is not found: empty finds it in Steam libraries and the usual folders. |
| Lossless Scaling: Let Lossless Scaling start by itself | `lossless_scaling_legacy_auto_detect` | `false` | Lossless Scaling starts scaling by itself (auto scale) and keeps its window open, instead of Rubylight pressing its hotkey. Try it if scaling does not start; apps can override it. |
| Commands around every app: Preparation commands | `global_prep_cmd` | none | Commands run before every app starts (do) and after it closes (undo); if a do command fails, the app does not start. Apps can opt out in their settings. |
| Commands around every app: Pause and resume commands | `global_state_cmd` | none | Commands run when any app's stream resumes (do) and when it pauses (undo), before the app's own state commands. Apps can opt out in their settings. |
| Commands from devices: PC commands | `server_cmd` | none | Commands a device can run on this PC from its stream menu, in streaming apps that support it. Only devices with the PC commands permission see them. Apps can add their own commands, with an undo, and hide these ([stream menu commands](#stream-menu-commands)). |

Earlier releases had one Sync automatically (`library_auto_sync`) and one Remove uninstalled games (`library_autosync_remove_uninstalled`) for every launcher. Rubylight gives each launcher and the game folders that value when it loads a configuration that sets them, unless the launcher's own key is set.

### Network

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Pairing: Allow pairing | `enable_pairing` | `true` | Let new devices pair with this PC. Devices already paired keep working either way; while it is off, no other device can ask to pair. |
| Discovery and internet: Local network discovery | `enable_discovery` | `true` | Announce this PC on the local network (mDNS) so Moonlight finds it without typing an address. Leave it on; off, you add this PC on each device by its address. |
| Discovery and internet: Automatic port forwarding | `upnp` | `false` | Ask the router to forward the streaming ports so devices can connect over the internet, through UPnP, PCP or NAT-PMP, and open the IPv6 firewall for them. The router must have one of them turned on. [Play over the internet](#play-over-the-internet) |
| Discovery and internet: Play over the internet | `internet_play` | `false` | Let paired Nightjar devices reach this PC from outside your network without port forwarding, over a direct path found through public STUN servers and a public peer-to-peer directory. Needs a restart. [Play over the internet](#play-over-the-internet) |
| Discovery and internet: STUN servers | `internet_play_stun_servers` | empty | Public STUN servers that tell the host its public address. Empty uses `stun.moonlight-stream.org:3478` and `stun.cloudflare.com:3478`. Needs a restart. |
| Streams: Local network encryption | `lan_encryption_mode` | `0` (Off) | Encrypts streams on the local network, Tailscale included. Off suits a network you trust: encrypting costs slower devices performance; Required refuses devices that cannot encrypt. |
| Streams: Internet encryption | `wan_encryption_mode` | `1` (When the device supports it) | Encrypts streams outside the local network. Leave it on When the device supports it: Off sends video over the internet unencrypted; Required refuses devices that cannot encrypt. |
| Streams: Device timeout | `ping_timeout` | `10000` ms | How long this PC waits without hearing from a device before it ends the stream. Longer rides out dropouts on unreliable networks, but a vanished device holds its stream that long. |
| Streams: Client roaming | `client_roaming` | `true` | Let a device whose network address changes, such as a phone moving from Wi-Fi to mobile data, keep its stream instead of reconnecting. Leave it on; other devices stream as before. |
| Streams: Prioritize stream traffic (DSCP) | `dscp_tagging` | `true` | Marks video and audio for local devices (DSCP) so Wi-Fi and switches that honour the marks send them first. Leave it on unless sound crackles or the picture stutters on one router. |
| Streams: Bandwidth test | `bandwidth_probe` | `true` | Lets paired devices time a short download to measure the network before they pick a bitrate. Leave it on: the device decides whether to run it, and off refuses the download. |
| WebRTC: WebRTC server | `webrtc_server` | `false` | Let TVs and browsers that speak the Moonlight WebRTC protocol find this PC, pair and stream. Leave it off unless you use one: its signalling (connection setup) is not encrypted. |
| WebRTC: WebRTC signalling port | `webrtc_port` | `8000` | The TCP port TVs connect their WebSocket to. Change it only if another program uses it; discovery always listens on UDP 8000, where TVs send their broadcast. |
| WebRTC: WebRTC media port | `webrtc_media_port` | `8001` | The UDP port for WebRTC video, audio and input. Change it only if another program uses it. |
| File transfer: File transfer | `file_transfer` | `false` | Let paired devices send files to this PC and download them again. Each device also needs the Upload files or Download files permission. |
| File transfer: File transfer folder | `file_transfer_dir` | empty | Where uploads go and downloads come from. Leave it empty for Downloads\Rubylight of the signed-in user. |
| File transfer: Largest file | `file_transfer_max_mb` | `4096` MiB | Uploads larger than this are refused before they start. |
| File transfer: Transfer speed while streaming | `file_transfer_stream_mbps` | `50` Mbit/s | Uploads and downloads together slow to this rate while a stream runs, leaving the rest of the network to the stream. 0 lets them run at full speed. |
| Shared folders: Shared folders | `shared_folders` | none | Folders of this PC that devices may browse. Each device sees only the folders you share with it on Devices. |
| Clipboard: Copy images | `clipboard_image` | `true` | Let devices with clipboard permission copy images to and from this PC, besides text. |
| Clipboard: Largest image | `clipboard_image_max_mb` | `16` MiB | Images larger than this, as PNG, are refused in both directions. |
| Ports and addresses: Base port | `port` | `47989` | Moonlight connects on this port, and Rubylight's other ports are counted from it. Change it only if another program needs these ports; this console's address moves with it. |
| Ports and addresses: IP versions | `address_family` | `ipv4` (IPv4 only) | Also accept devices that connect over IPv6. Leave it on IPv4 only unless devices must reach this PC over IPv6, such as over the internet; ignored when a listen address is set. |
| Ports and addresses: Listen address | `bind_address` | empty | Devices can reach this PC only at this one IP address, which must be one of this PC's own; local network discovery uses it too. Leave it empty to listen on every address. |

### Advanced

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Logging: Log level | `min_log_level` | `info` (Information) | How much detail Rubylight writes to its log; changes apply right away. Leave it on Information; Debug suits a problem report. |
| Logging: Log file | `log_path` | `logs/butterpollo.log` | Where Rubylight writes its log. A relative path is inside Rubylight's configuration folder. |
| Compatibility: App order for older devices | `legacy_ordering` | `false` | Turn on only if an older Moonlight lists your apps in the wrong order: it adds invisible characters to app names, which can break scripts and tools that match names exactly. |
| Compatibility: Whole-number frame rate for commands | `envvar_compatibility_mode` | `false` | Give commands SUNSHINE_CLIENT_FPS as a whole number, such as 60 instead of 59.940. Turn it on if a script cannot read fractional frame rates. |
| Console: Console access | `origin_web_ui_allowed` | `lan` (Local network) | Which addresses may open this console and use its API. Leave it on Local network: Any network also accepts addresses on the internet, and This PC only shuts out your other devices. |
| Console: Trusted origins | `csrf_allowed_origins` | none | Other web addresses allowed to make changes through this console. Add one only when you reach the console through a reverse proxy. |
| Console: Sign-in token lifetime | `session_token_ttl_seconds` | `7200` s | How long a console sign-in token is valid before the browser renews it on its own. Leave it at 2 hours unless you want tokens replaced more or less often. |
| Console: Remembered sign-in lifetime | `remember_me_refresh_token_ttl_seconds` | `604800` s | How long a browser signed in with "Keep me signed in" stays signed in, up to 366 days. Lower it on shared computers. |
| Debug switches: Lower the bitrate when the link cannot carry it | `link_rate_adaptation` | `true` | Always on; listed under Debug switches for troubleshooting only. Cuts the picture bitrate when the network loses more than error correction can repair, then probes back up; does nothing on a healthy link. See [Lower bitrate when the link cannot carry it](#lower-bitrate-when-the-link-cannot-carry-it). |
| Debug switches: Pair with a QR code | `qr_pairing` | `true` | Always on; listed under Debug switches for troubleshooting only. Shows a QR code on Home and the Devices page that Nightjar scans to pair, instead of typing a PIN. The code holds a one-time PIN and passphrase that work once and expire after 3 minutes, and the fingerprint of the host certificate, which Nightjar checks before pairing. |

## Further reference

- [Features](features.md)
- [Measured performance and testing conditions](../rust/PERFORMANCE.md)
- [Console settings definitions](../rust/web/src/lib/settings-schema.ts), including [video](../rust/web/src/lib/schema/video.ts), [display](../rust/web/src/lib/schema/display.ts) and [general settings](../rust/web/src/lib/schema/basics.ts)
- [Configuration parser](../rust/core/src/config.rs) and [allowed overrides](../rust/core/src/config/overrides.rs)
- [Historical C++ configuration reference](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/configuration-cpp.md) — preserved for older installations; its defaults and platform advice do not describe the current Rust host.
