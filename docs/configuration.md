# Configuration

[Documentation](README.md) · [Getting started](getting-started.md) · [Troubleshooting](troubleshooting.md)

Rubylight is configured in its web console, at **https://localhost:47990** on the host PC. The client chooses the resolution, frame rate, bitrate, codec and HDR of each stream; the host's settings decide how it captures, encodes, paces and delivers what the client asks for. The defaults suit a typical setup, and upgrades keep the values you have saved.

This guide covers each feature's settings by topic. [All settings](#all-settings) at the end lists every setting in the console with its configuration key and default.

| Area | Sections |
| --- | --- |
| Basics | [How configuration works](#how-configuration-works) · [Defaults in 3.0.0](#defaults-in-300) · [Stream presets](#stream-presets) |
| Picture and displays | [Capture and video](#capture-and-video) · [Displays and RTSS](#displays-and-rtss) |
| Network | [Lower bitrate when the link cannot carry it](#lower-bitrate-when-the-link-cannot-carry-it) · [Host bitrate adaptation](#host-bitrate-adaptation) · [Error correction chosen by the device](#error-correction-chosen-by-the-device) · [Roaming](#roaming) |
| Sound | [Game-only audio](#game-only-audio) · [PCM and Dolby Digital audio](#pcm-and-dolby-digital-audio) · [Surround, night mode and headphone surround](#surround-night-mode-and-headphone-surround) |
| Input | [Keyboard, touch, pen and cursor](#keyboard-touch-pen-and-cursor) · [Controllers](#controllers) · [MoonDeck](#moondeck) · [Couch menu](#couch-menu) |
| Games | [Close apps on disconnect](#close-apps-on-disconnect) · [Playnite launch focus](#playnite-launch-focus) · [Radeon settings per game](#radeon-settings-per-game) · [Managed game files](#managed-game-files) · [Neural Rendering](#neural-rendering) · [Background streaming](#background-streaming) |
| Apps and devices | [Settings for one app or device](#settings-for-one-app-or-device) · [Host control, session actions and Radeon settings](#host-control-session-actions-and-radeon-settings) |
| Sharing and browsers | [Clipboard, files and USB](#clipboard-files-and-usb) · [File transfer](#file-transfer) · [Shared folders](#shared-folders) · [WebRTC server](#webrtc-server) |
| Running the host | [Multiple instances](#multiple-instances) · [Updates](#updates) |
| Reference | [All settings](#all-settings) · [Further reference](#further-reference) |

## How configuration works

The **Settings** page shows the ten settings most people change, in five sections: **This PC** (host name and **Allow pairing**), **Picture** (stream preset, virtual display, and what your monitors do while you stream), **Sound and controls** (stream audio, the device microphone and controllers), **Games** (closing the app when the last device disconnects) and **Updates** (installing updates automatically).

**All settings**, under **Advanced** in the sidebar, has every setting in six tabs: **General**, **Streaming**, **Display & HDR**, **Audio & controls**, **Games & automation**, and **Network & access**. Groups that matter only for troubleshooting (Capture troubleshooting, AMD AMF, RTSS, Ports and addresses, Console, Debug switches) start collapsed, and **Show config keys** shows each setting's configuration key. Paths in this guide name the page, the tab and the setting, such as **All settings → Streaming → Capture method**.

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

These keys are read from the configuration file (`sunshine.conf` in the profile folder) and from device and app overrides, but have no field in the console:

| Key | Default | What it does |
| --- | --- | --- |
| `audio_night_mode`, `audio_headphone_surround` | `false` | See [Surround, night mode and headphone surround](#surround-night-mode-and-headphone-surround). |
| `couch_menu` and the `couch_menu_*` keys | `true`, and the values under [Couch menu](#couch-menu) | How the couch menu opens. |
| `video_slice_delivery` | `off` | Sends H.264 and HEVC P-frames in 2 or 4 FEC blocks as their slices leave the encoder. See [Capture and video](#capture-and-video). |
| `webrtc_pacing`, `webrtc_rtx` | `true` | Send pacing and retransmission for the WebRTC server. |
| `webrtc_loss_permille` | `0` | Drops WebRTC video packets on purpose, in thousandths, for tests. Leave it at 0. |

Webhooks have their own console page; see [Webhooks](webhooks.md).

## Defaults in 3.0.0

<a id="defaults-worth-knowing"></a>

Each default below is what a fresh 3.0.0 install uses, checked against the host code. The stream preset **Balanced** is exactly this set; nothing in the table needs changing for a normal setup.

| Setting — key | Default | Why |
| --- | --- | --- |
| **Capture and encoding** | | |
| Capture method — `capture` | `auto` | On a virtual display Rubylight created, each frame comes straight from Rubylight's display driver, with no capture copy. Elsewhere Windows Graphics Capture (WGC), with Desktop Duplication (DDX) when WGC is unavailable. |
| Copy and convert on a compute queue — `gpu_compute_conversion` | `true` | Copy and colour conversion run on a compute queue beside the game instead of waiting behind its graphics work ([October 4](../rust/PERFORMANCE.md#october-4-capture-copies-and-conversion-beside-a-game)). |
| Encode from GPU memory — `wgc_direct_encoder_input` | `true` | Captured frames stay on the GPU; nothing goes through system memory. |
| Encoder — `encoder` | `auto` | AMD AMF for H.264, HEVC and AV1, and PyroWave's own encoder for PyroWave. There is no other encoder and no fallback, so a failure is reported instead of hidden. |
| Stream preset — `stream_preset` | `balanced` | Balanced is every default in this table. See [Stream presets](#stream-presets). |
| Usage — `amd_usage` | `ultralowlatency` | AMD's lowest-latency profile; the driver also turns on its internal low-latency mode, and the lowest latency for AV1. The other profiles queue frames. |
| Quality preset — `amd_quality` | `speed` | The lowest encode time. With these two defaults the encode stage of a 1080p60 AV1 stream took a median 2.26 ms, and the whole path from capture to the client's present 5.89 ms, on loopback ([October 26](../rust/PERFORMANCE.md#october-26-300-rc2-against-the-published-numbers)). |
| Long-term reference frames — `amd_ltr_frames` | `4` | A device that lost frames recovers from a reference it still has, on H.264, HEVC and AV1, and the host sends a keyframe only where that would be smaller. Over the measured gaps that sent 0.86 to 0.92 of the keyframe bytes ([October 28](../rust/PERFORMANCE.md#october-28-ltr-recovery-on-h264-and-hevc-with-header-rewrites-on-by-default)). |
| Maximum frame size — `amd_max_frame_size` | `auto` | Keeps every frame inside what error correction can still protect, and encodes a scene cut that comes out too large again as a keyframe. Lost scene cuts went from 16 and 15 to 0 and 0, and freezes from 15 to 0 ([October 28](../rust/PERFORMANCE.md#october-28-oversize-scene-cuts-encoded-again-as-keyframes)). |
| Encoder focus — `encoder_focus` | `off` | Favouring still areas makes scene cuts larger, so it is a choice for text-heavy desktops, not a default. See [Encoder focus](#encoder-focus). |
| HEVC / AV1 support — `hevc_mode`, `av1_mode` | `0` (Automatic) | Profiles the encoder fails at host start are never offered. |
| PyroWave — `pyrowave` | `true` | Offered only to devices that ask for it; others never see it. |
| **Frame rate and pacing** | | |
| Frame pacing — `frame_pacing` | `arrival` | Each new frame is encoded as soon as it is captured, limited to the requested stream rate. |
| Use the launch frame rate — `limit_framerate` | `false` | The stream runs at the rate the stream request asks for, which is what Artemis's Warp frame pacing expects. |
| Pacing rate — `pacing_max_bitrate_kbps` | `0` (automatic) | Fast wired pacing, twice the encoder bitrate on a host Wi-Fi route, and a device's reported link speed caps it. |
| Adaptive packet pacing — `host_adaptive_pacing` | `false` | Starts slower and probes; useful only on a link with burst loss that a fixed rate cannot fix. |
| Adaptive packet pacing for Wi-Fi clients — `wireless_adaptive_pacing` | `true` | A device that says it is on Wi-Fi or cellular gets that controller from the first frame. On a real laptop over Wi-Fi this cut unrecoverable frames from 61 to 15.7 ([October 25](../rust/PERFORMANCE.md#october-25-wi-fi-pacing-hint-on-a-real-laptop)). |
| Slow down after video loss — `keyframe_loss_pacing` | `true` | Only loss that lasts slows packet delivery; the bitrate stays. |
| Lower the bitrate when the link cannot carry it — `link_rate_adaptation` | `true` | Always on. Acts only on loss error correction cannot repair, so a healthy link never changes. See [Lower bitrate when the link cannot carry it](#lower-bitrate-when-the-link-cannot-carry-it). |
| Adapt bitrate on the host — `host_adaptive_bitrate` | `false` | Devices that control their own bitrate do it better from their side; this is for clients that do not. |
| Most error correction a device may ask for — `fec_max_percentage` | `50` % | A device that chooses its own error correction may go up to half the video packets. In the October 27 runs this cut Wi-Fi loss and freezes against the host's own share ([October 27](../rust/PERFORMANCE.md#october-27-client-error-correction-and-a-link-test-before-the-first-bitrate-nuc)). |
| Bandwidth test — `bandwidth_probe` | `true` | Lets a paired device time a short download before it picks a bitrate. The device decides whether to run it. |
| Virtual display refresh — `frame_limiter_auto_virtual_framegen` | `legacy` | The virtual display runs at twice the stream rate, so each game frame is captured sooner after it is drawn, and automatic frame limiting applies. |
| Limit every stream — `frame_limiter_enable` | `false` | Physical monitors keep the game's own frame rate; virtual displays are limited automatically. |
| Limiter — `frame_limiter_provider` | `auto` | RTSS when it is installed. |
| **Displays and HDR** | | |
| Virtual display — `virtual_display_mode` | `per_client` on Windows 11 23H2 or later, `disabled` on earlier Windows | Each device gets a display at its own resolution and refresh. Earlier Windows lacks the driver interface the virtual display needs. |
| Virtual display layout — `virtual_display_layout` | `exclusive` | The other monitors turn off while you stream, so games open on the streamed display. See [Displays and RTSS](#displays-and-rtss). |
| Monitor setup — `dd_configuration_option` | `verify_only` | A physical monitor is only checked to be on; its mode is left alone. |
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
| Internet encryption — `wan_encryption_mode` | `1` (When the device supports it) | Streams leaving your network are encrypted whenever the device can. |
| Client roaming — `client_roaming` | `true` | A phone moving from Wi-Fi to mobile data keeps its stream. See [Roaming](#roaming). |
| Console access — `origin_web_ui_allowed` | `lan` | The console and its API answer only on your local network. |
| **Permissions and sharing** | | |
| USB forwarding — `usb_forwarding` | `false` | A USB device on the PC is a real device; you choose to allow it, and each device needs its own permission. See [Clipboard, files and USB](#clipboard-files-and-usb). |
| File transfer — `file_transfer` | `false` | Off until you turn it on; each device also needs its own permission. |
| Shared folders — `shared_folders` | none | Nothing is shared until you add a folder and give it to a device. |
| Copy images — `clipboard_image` | `true` | Devices you allow to use the clipboard get images as well as text. |
| Host control, Sleep PC, USB devices, files and clipboard, per device | off | Granted per device under **Devices**; pairing never grants them. |
| Can join co-op games, per device — `coop_join` | follows **Launch apps** | Joining a co-op game launches it, so a device that may launch apps may join until you take it away. |
| WebRTC server — `webrtc_server` | `false` | Only TVs and browsers that speak the Moonlight WebRTC protocol need it. |
| **Games, power and updates** | | |
| Close the app when the last device disconnects — `terminate_on_pause` | `false` | The game keeps running so you can reconnect. |
| Steam library — `steam_enabled` | `true` | Steam games appear with their covers. Every other launcher you install adds its games the same way. |
| Co-op players — `nucleus_coop_players` | `4` | The game's handler and the free controller slots can allow fewer. |
| Sleep after idle — `sleep_after_idle_minutes` | `0` (off) | The PC sleeps only when you ask it to. |
| Update check interval — `update_check_interval` | `86400` seconds | Once a day. |
| Install updates automatically — `auto_update` | `false` | You are told about an update and install it when you choose. |
| Include pre-releases — `notify_pre_releases` | `false` | Release candidates are offered only when you ask for them. |

These are defaults for **unset** keys, not a list of values you must copy into a configuration file.

Under **All settings → Streaming**, the collapsed **Capture troubleshooting** group holds the capture options that matter only when a capture or encoder problem needs isolating: `capture_poll_interval_us`, `capture_predictive_poll`, `wgc_slot_aligned_publish`, `wgc_direct_encoder_input`, `gpu_compute_conversion` and `idd_push_fence_ring`. **Use the launch frame rate** (`limit_framerate`) and **Follow client size and frame rate changes** (`stream_reconfigure`), in the **Frame pacing** group, matter only to Artemis's Warp frame pacing and to Rubylight clients. `minimum_fps_target` is **Still-picture resend rate**: how often an unchanged picture is sent again, not a floor on a game's frame rate.

When several bitrate and pacing controllers are on, **Adapt bitrate on the host** (`host_adaptive_bitrate`) wins over **Lower the bitrate when the link cannot carry it** (`link_rate_adaptation`), and a device that sets its own bitrate takes over from both. A manual **Pacing rate** (`pacing_max_bitrate_kbps`) turns off **Adaptive packet pacing** and **Slow down after video loss**; **Adapt bitrate on the host** turns off both of those too, and **Adaptive packet pacing** turns off **Slow down after video loss**.

## Stream presets

`stream_preset` names a set of defaults for three AMD encoder settings: Lowest latency (`lowest_latency`), Balanced (`balanced`, the default) and Reference quality (`reference`). A preset only fills the covered settings that nobody set. A value in the settings file, in a device's overrides or in an app's overrides always wins, so you can pick a preset and still change one setting by hand. The preset's values are not written to the file; changing or clearing the preset changes the ones you did not set. Saving a covered setting in the console stores it, even if it equals the preset's value, so it stays when you pick another preset.

| Setting | Lowest latency | Balanced | Reference quality |
| --- | --- | --- | --- |
| `amd_max_frame_size` | `1` (one frame's share of the bitrate) | `auto` | `0` (driver default, no limit) |
| `amd_quality` | `speed` | `speed` | `speed` |
| `amd_preanalysis` | `false` | `false` | `false` |

- **Lowest latency** limits every frame, recovery keyframes included, to one frame's share of the bitrate. In [the October 7, 2026 recovery run](../rust/PERFORMANCE.md#october-7-2026-amf-rate-control-and-recovery-keyframes) a one-frame cap cut the largest 4K40 HEVC recovery frame by 66% with the same steady frame sizes and encode time. It names the cheapest quality preset and pre-analysis off as well, which are the defaults, so it changes nothing there. A value you set for either still applies and adds its measured cost (1.0 ms for HEVC and 12 ms for AV1 per 4K60 frame for the quality preset; 25 ms against 5-6 ms for pre-analysis).
- **Reference quality** removes the size limit on recovery keyframes (`amd_max_frame_size=0`, the driver default). No picture quality gain from that has been measured, so the preset claims none. What was measured is the cap itself: in the same recovery run a one-frame cap cut the largest 4K40 HEVC recovery frame by 66% and the encoded payload from 64 to 51 Mbps in the 63 Mbps run. Without the cap, recovery keyframes are larger on the wire, so use it on a fast, steady link. That run measured HEVC; H.264 with a one-frame cap was not measured separately (H.264 and AV1 were only checked at a two-frame cap, for compatibility). Pre-analysis, the quality presets and the quality rate-control modes stay off: the measurements show no consistent picture gain for their encode time. Packed 4:4:4 is not part of any preset: on a Radeon 780M (8-bit HEVC) it improved still text and UI above about 5 Mbit/s of actual bitrate, but scrolling at 5 Mbit/s was 0.8 dB worse and the first frame was worse below 20 Mbit/s; it added 1.2 to 2.1 ms of encode time and 0.7 to 1.2 ms of hardware decode, and was worse at 2 Mbit/s, so it stays the separate **Full chroma** choice ([measurements](../rust/PERFORMANCE.md#october-23-420-against-packed-444-on-text-and-ui-and-afmf-capture-b3-b7-measured-on-the-780m)).
- **Why no preset sets `amd_rc`.** Setting it makes the encoder require that mode and fail loudly if it is unavailable. Constant bitrate is not a quality setting either: in the [AMF quality sweep](../rust/PERFORMANCE_WORK.md#amf-settings-per-codec-quality-at-a-given-bitrate) it scored slightly higher on the first clip only because it spends more of the budget, the 99th-percentile frame grew by 30-67%, and the 48 HDR encodes found no reason to change the rate control. Rate control stays at the default in every preset.

The console shows the selector under **Streaming** and, below it, which covered settings have a value of their own ("Custom"), including one equal to the preset's. Settings the preset covers show its values as their defaults, and Reset returns to them. A name the host does not know still loads, and the host uses Balanced; the console shows the stored name with an error instead of showing Balanced.

A device or an app can name its own preset with `stream_preset` in its overrides (`config_overrides` for a device, `config-overrides` for an app). The most specific name wins (an app's settings for the device, then app, then device, then host; see [one game on one device](#one-game-on-one-device)), and it fills only the three settings that no layer set: an explicit value of the host, the device or the app is never replaced by a preset. The host refuses to save a device or app preset name it does not know, or one that is not text. The device editor has a **Stream preset** control for the device's own name; the app editor sets it through the app's overrides, and for one device in its **Per device** panel.

## Capture and video

**Graphics card** (`adapter_name`) lists all hardware GPUs reported by Windows DXGI, including GPUs with no attached monitor on hybrid systems. **Automatic** uses the GPU that owns the captured display. A GPU appearing in the list does not confirm encoder or codec support; that still depends on its hardware and driver. With a GPU selected, virtual displays are created to render on that GPU. WGC can capture a display that another GPU renders; Desktop Duplication cannot, so when it is the capture path in use and the display renders elsewhere (a physical monitor on the integrated GPU, or a driver that ignored the request), the stream captures and encodes on the display's GPU and the stream card says so.

**Keep Capture method on Automatic and compute conversion enabled** for normal use. On supported AMD hardware, the compute path lets capture and colour conversion run alongside graphics work. Unsupported textures can fall back to the graphics path. The internal `wgc_compute_copy` switch also defaults to `true`; it is primarily useful for controlled troubleshooting.

**Frames from Rubylight's display driver.** On a virtual display Rubylight created, Automatic captures from the driver itself: it copies each frame Windows composes into textures it shares with the host, so neither WGC nor Desktop Duplication runs, and on AMD the compute conversion reads those textures in place (`idd_push_zero_copy`, default `true`). To select it explicitly, set **All settings → Streaming → Capture method** to **Direct driver push** (`capture = idd-push`). It applies to Rubylight's own virtual displays and falls back to WGC when unavailable, including on a physical monitor. Choosing WGC or Desktop Duplication explicitly requests those methods. The stream captures with WGC instead, and the stream card says why, when the driver is not installed, when another stream with different capture settings already captures that display, or when the display's frame push failed twice before delivering a picture. A display that failed is tried again once it is recreated or renders on another GPU. Save and end all streams using the display before starting a new session to compare capture methods; simultaneous streams may share an already-open capture.

**GPU fence transport** (`idd_push_fence_ring`, default `true`) is the transport for direct driver push on AMD with compute conversion and in-place capture enabled. It uses D3D12-owned shared textures and GPU fences while the display driver continues using D3D11. Set it to `false` to use the keyed-mutex ring instead. It requires a compatible Rubylight display driver; missing support, failed setup or failed synchronization uses the keyed-mutex ring and shows a warning. A rejected fence transport stays rejected through capture restarts on that display. Recreate the virtual display or restart the host to retry. Save and end all streams using the display before changing it. WGC and Desktop Duplication remain available. This option does not require the new native D3D12 IddCx swap-chain API.

**Downscaling uses an antialias filter automatically.** Supported GPU conversion paths average each output pixel's covered source pixels for reductions up to 8× per axis. This helps when the desktop and stream sizes differ, including fractional reductions. Native resolution, enlargement and aligned 2× reduction keep the existing filter. Larger reductions retain the previous filter to bound GPU work. Existing HDR transfer handling, cursor composition and aspect-ratio handling remain in the same conversion pass.

The installed service runs WGC through a helper in the signed-in user's session. WGC can fall back to DDX when unavailable. Selecting a particular capture method is useful when investigating a problem; check **Logs** to see which path actually opened.

H.264, HEVC and AV1 depend on the encoder and client. For `hevc_mode` and `av1_mode`, the choices are `0` Automatic, `1` Off, `2` SDR only and `3` SDR and HDR. Restart after changing advertised codec support. AMD's encoder makes 4:2:0 only, so stock Moonlight gets 4:4:4 from an AMD host through PyroWave alone. For full chroma over HEVC or AV1, turn on **Full chroma** (`experimental_packed_444`, off by default) under **All settings → Streaming → Codecs**, restart the host, and turn on the matching option in Nightjar. The host then packs each 4:4:4 frame into an ordinary 4:2:0 picture and Nightjar puts it back together; stock Moonlight never sees the packed layout.

For **native HDR**, enable HDR in the client and use an HDR-capable display and encoder path. Leave **Display HDR** on Automatic so Rubylight can set the source display appropriately. **10-bit SDR instead of HDR** (`prefer_sdr_10bit`, default `false`) deliberately keeps the stream in SDR; leave it off when you want HDR. A client can also ask for 10-bit SDR itself: when it negotiates a 10-bit format (`dynamicRangeMode=1`) and sends `hdrMode=0` (on /launch or /resume), Rubylight streams 10-bit SDR and leaves the display in SDR, whatever this option says. Without `hdrMode`, or with `hdrMode=1`, a 10-bit format stays HDR unless `prefer_sdr_10bit` is on.

**HDR dynamic metadata** (`hdr_dynamic_metadata`, default `true`) adds per-frame brightness statistics to HDR streams: HDR10+ (SMPTE ST 2094-40) in HEVC and AV1, and on HEVC also HDR Vivid (CUVA 005.1). Displays that tone-map with HDR10+ can then follow each scene; clients without it ignore the metadata. HDR10+ is written as profile A, without a target display, and the first frames, before any measurement, carry values from the stream's static HDR10 levels. They are measured on the GPU from the captured picture while the frame encodes, behind the compute conversion (`gpu_compute_conversion`) and the D3D11 conversion alike; PyroWave carries none. Packed 4:4:4 streams carry static HDR10 only, from the control channel: the device unpacks the picture in its own shader, so no decoder would read the statistics, and the host logs that and runs no statistics pass. If the statistics pass does not start or fails during a stream, the stream goes on with static HDR10 only and the web UI says why. Static HDR10 metadata is the same either way. One switch covers both formats: turn it off if a display handles brightness worse with it.

**HLG** (`hdr_hlg`, default `true`) offers HLG (ARIB STD-B67) on HEVC, AV1 and PyroWave HDR streams to devices that ask for it (`HdrFormats` in serverinfo, `x-rl-video.hdrFormats` in DESCRIBE, only while an HEVC, AV1 or PyroWave Main10 encoder is there; PyroWave adds the offer flag 128, since older hosts refuse HLG on PyroWave). The host then encodes the captured picture with the BT.2100 HLG curve, graded for a fixed 1000-nit display, and signals transfer 18 in the bitstream; the device's screen tone-maps from there, and light above 1000 nits keeps the little headroom the signal has before it clips. Devices that do not ask, stock Moonlight among them, get HDR10 as before. HEVC HLG streams carry HDR Vivid statistics while **HDR dynamic metadata** is on, capped at 1000 nits like the picture; HDR10+ describes HDR10 streams only, and AV1 has no HDR Vivid. Packed 4:4:4 streams stay HDR10, and the host logs why.

**Dolby Vision** (`hdr_dolby_vision`, default `true`) offers Dolby Vision profile 8.1 on HEVC HDR streams and profile 10.1 on AV1 HDR streams to devices that ask for it (`HdrFormats` in serverinfo, `x-rl-video.hdrFormats` in DESCRIBE, only while an HEVC or AV1 Main10 encoder is there), and profile 8.4 on HEVC while **HLG** is on. Each frame then carries a Dolby Vision RPU built from the same per-frame statistics as HDR10+, over the stream's HDR10 base layer (HLG for 8.4): on HEVC in a NAL unit at the end of the frame, on AV1 in a metadata OBU before it. Devices without Dolby Vision and stock Moonlight see the HDR10 or HLG stream underneath. Until the first sample, or if the statistics stop, the RPUs carry the stream's static light levels and the web UI says so. Packed 4:4:4 streams carry static HDR10 instead: the device unpacks the packed picture itself, so a Dolby Vision decoder would tone-map the packed layout; the host logs this and the web UI shows the reason.

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

**Repeated video loss slows packet delivery** (`keyframe_loss_pacing = true`). The guard activates when the client reports a keyframe it could not repair, asks again for the keyframe it was just sent, or reports three distinct unrecoverable frames within two seconds. A client that announces FEC status (`x-ml-general.featureFlags` 0x01: Moonlight, Artemis, Nightjar) reports every frame it lost, so its repeated keyframe request is its decoder asking again, not loss: it is logged as "video loss evidence not counted" and does not activate the guard. Frames the host could not send whole because the network send buffer was full are loss too: three of them within two seconds, or one keyframe, activate it, named "lost on send" in the log, and the client's report of the same frame is not counted a second time. When only the host saw that loss (the host's network adapter stalled, the client reported nothing), the slower pace lifts after 10 clean seconds and 120 frames like a lost first keyframe; more of it while the pace is slow restarts the 10 seconds. In a stream that already slowed down once (an earlier lift, or a route remembered from a previous session) it holds for the rest of the stream at once. Loss only the host saw is never saved, and a repeated request for a keyframe the host's socket refused is the host's loss too. Reports for several blocks of the same frame count once. Nothing the host did itself counts: a keyframe the host sent on its own (Alt+Tab, a bitrate change, a restarted capture or encoder) is not a keyframe the client "asked again" for, even if the client asked just before it, and requests and ordinary-frame reports within one second after it are set aside. The same goes for the first half second after the stream's first keyframe (a client that has not shown a picture yet asks again within about 100 ms). Keyframes sent to recover from loss open no such window. The log says what set it off (the frame numbers) and what was set aside. Stale ordinary-frame reports, isolated losses and fully repaired frames do not trigger it. The stream is then paced like a confirmed Wi-Fi route for the rest of the session: twice the encoder bitrate, at least 1.25 times wire demand, within the route ceiling. The encoder bitrate does not change. With the default sender thread enabled, loss feedback received before sender selection moves the next output to that thread, avoiding a slow inline send on capture.

Sustained loss the client reported is saved with the paired device's authenticated identity, its IP address, the host network interface the route leaves by, and the client's own link speed when it reports one (Nightjar does on wired links), at stream end. Loss that was only the stream's first keyframe, with nothing else seen, holds the slower pace for that stream until it has been clean for 10 seconds, and is not saved; a second latch in the same stream, or more loss the client reports while it holds, is sustained (loss only the host saw while it holds restarts the 10 seconds). The next eligible session at that address starts at the slower pace, including its first recovery frame; the one after tries the ordinary route pace again. A different host interface (another network adapter; the adapter is told apart, not the network it is joined to) or a different client link starts without history, and records saved by earlier releases are ignored. A completed probe with at least five seconds and 120 sent frames and no new loss clears that route's history. Entries expire 24 hours after observed loss; simply reusing remembered pacing does not extend that lifetime. **Devices → Reset network history** clears the device's stored routes for subsequent streams, without changing a running stream. A different address starts independently. A client that reports its wired link (`x-ss-video[0].pyrowaveLinkMbps`) caps conventional-codec pacing at 4/5 of the slower of the two network ports from the first packet: a 1 GbE client stays at 800 Mbps, a 100 Mbps client gets 80. A hint below the encoder bitrate does not pace the stream under it: the pace is then 1.1 times the bitrate, and the host log carries one warning naming both values. Only the stream's first keyframe, and a keyframe the client asks for within a second of it, count as the start; a keyframe lost later, such as one from an encoder rebuild, is not provisional. The 10 clean seconds are counted from the last loss. Manual pacing, either adaptive controller, disabled loss pacing, and PyroWave do not use this memory.

Adaptive error correction keeps the configured protection for 30 seconds after loss, then decays by one percentage point every five seconds of clean feedback. At the default 20% ceiling it reaches the 5% floor after 100 clean seconds. Recovery frames retain the ceiling, and clients without FEC feedback retain the configured percentage.

The original keyframe guard was measured through a 100 Mbps loopback bottleneck with a 16 or 64 KiB queue: 1440p60 AV1 and HEVC streams at 40 and 60 Mbps went from no frame received whole in 20 seconds to all but the first few. Those historical measurements do not measure the newer repeated-frame trigger or remembered routes. [Measurements](../rust/PERFORMANCE.md#october-14-keyframe-loss-on-a-slow-port).

**Adaptive packet pacing is off by default** (`host_adaptive_pacing = false`). It is an option for connections with burst-related loss when manual pacing is inconvenient. Enabling it starts each conventional stream at twice the encoder bitrate or 1.25 times wire demand, whichever is higher, within the route ceiling. A suitable fixed limit is simpler when the constrained path is known. Neither policy establishes the cause of an Onn client's reported problem; that device has not been validated.

**Wi-Fi clients get it automatically** (`wireless_adaptive_pacing = true`). The host sees only its own link, so a wired host cannot tell that the client sits behind an access point. A client that reports a Wi-Fi or cellular route to the host with the optional SDP attribute `x-rl-video[0].clientWireless:1` (Nightjar does; absent or `0` means unknown and changes nothing) gets the adaptive controller below on H.264, HEVC and AV1, even though `host_adaptive_pacing` is off. The controller starts at twice the encoder bitrate (at least 1.25 times wire demand), so the first keyframe is not sent as a line-rate burst. It then probes up in steps of at most 12.5% while the link stays clean, up to the host route's ceiling, and backs off on loss, exactly as for `host_adaptive_pacing`. Keyframe-loss slowdown and its remembered routes are off for such a stream, because the controller owns pacing, and the encoder's keyframe size cap follows the controller's rate as it does for `host_adaptive_pacing`. Where the controller does not own an H.264, HEVC or AV1 stream (`wireless_adaptive_pacing = false` or host adaptive bitrate), the hint instead applies the Wi-Fi route policy from the first frame (twice the encoder bitrate, at least 1.25 times wire demand, no probing), which also gives the smaller keyframe cap of a Wi-Fi host. A manual pacing limit always wins; the hint changes nothing there. For PyroWave the hint only stops the client's wired-link hint from setting the starting rate, so it starts from the demand-based rate instead, and a pace the client calibrated still wins.

During startup, busy video, recent standard client pings and stable nonzero RTT allow tentative increases of up to 12.5%, separated by at least half a second and a feedback-delay guard. A new loss report during a probe restores the preceding rate and switches to slower recovery. There is no timer-only jump to the maximum. Each reconnect starts conservatively again: a client can move between Ethernet and Wi-Fi without changing its identity or the host's route. Startup pacing adds transmission time, especially to a large first keyframe; with a 40 Mbps encoder and continuously healthy traffic, reaching an 800 Mbps ceiling takes about ten seconds.

Outside tentative probes, the optional controller reduces bursts after repeated, validated client FEC reports show substantial loss. It changes packet spacing without lowering encoder bitrate or FPS. A half-second sample needs two distinct failed frames, or multiple recovered frames with at least 2% of sent packets missing. Old and duplicate reports cannot repeatedly penalize a replacement rate. The rate stays at least 25% above current packetized video demand, including FEC, minimum parity, encryption and network headers, unless the route ceiling is already lower. This floor prevents pacing itself from creating a persistent backlog; it cannot make an overloaded path carry the stream.

Recovery needs recent standard client pings, active video traffic, stable RTT and an initial eight-second clean hold. Small rate increases are tentative; a failed probe restores the previous rate and extends the next recovery hold, up to a minute. Paused video, absent standard pings or missing RTT does not authorize a faster rate. Stock Moonlight does not acknowledge every successfully received video frame: control pings establish activity, not video-path capacity. Loss during probing remains possible, and clients without loss reports cannot trigger loss-based rollback. The session API exposes `packet_pacing`, including startup state, and rate changes appear in the host log. The separate **Host adaptive bitrate** option keeps its existing opt-in quality/bandwidth controller and disables this additional burst controller.

Conventional video batches contain at most about one millisecond of traffic at the selected rate, or one datagram if it takes longer. `video_max_batch_size_kb` remains an additional payload limit (16, 32 or 64 KiB; default 64). With `video_send_thread` enabled (the default), adaptive packet pacing reserves the dedicated video sender from the start so pacing waits do not block capture. Large keyframes can still take multiple frame periods. Startup and steady-state timing are reported separately in the measurement record.

`video_slice_delivery` (`off`, `2` or `4`; default `off`) sends an H.264 or HEVC P-frame in that many FEC blocks, each as soon as the encoder's slices for it have left AMF, instead of after the whole frame. It is set in the configuration file only, not in the console. Keyframes, recovery frames and AV1 always go out whole, and a stock Moonlight or Nightjar client reassembles the frames unchanged. Turning it on asks AMF for slice output with at least one slice per block and turns intra refresh off. A stream with its own send thread (current or adaptive pacing) streams the same way: the packetizer thread builds each block as the units arrive and the paced sender sends it. If a frame ever passes the byte cap its blocks were planned for, the host logs a warning, counts it in `slice_delivery_overflows`, requests a recovery frame and sends whole frames for the rest of the stream. `BUTTERPOLLO_TEST_SEND_LOSS=percent:seed[:air]` drops video datagrams at random for tests; unset it otherwise.

Set `pacing_max_bitrate_kbps` to a positive value in **kbps** to override automatic burst control, including for PyroWave. For example, `120000` paces at 120 Mbps. Conventional video retains its floor of 110% of the stream bitrate and ceiling of 80% of a known Ethernet link. PyroWave treats the setting, the client's calibrated pace and 95% of either endpoint's reported link as hard ceilings: stream demand cannot raise them. The host warns if the wire budget exceeds the resulting rate. Reconnect after saving. [Measurements and pacing math](../rust/PERFORMANCE.md#october-14-conventional-video-pacing).

PyroWave starts unmeasured and wireless paths at 1.25 times their estimated wire demand (at least 110% of the stream bitrate before that headroom), within the ceilings above. A wired host alone does not select line-rate bursts. A client reporting its wired link can retain fast wired pacing. Opt-in adaptive packet pacing also applies to PyroWave; the loss guard and Wi-Fi adaptive pacing do not. Loss that error correction cannot repair lowers PyroWave's bitrate as for every codec, but not below its severe-loss floor (see [Lower bitrate when the link cannot carry it](#lower-bitrate-when-the-link-cannot-carry-it)). The pace comes down with it only where it follows the stream's demand: a client pace hint, `pacing_max_bitrate_kbps` or a client's reported wired link keeps setting it. Calibration and streaming use the same segmented sends, retry handling and batches of at most one millisecond of wire traffic, 64 packets and 65507 payload bytes. Error correction can adapt up to `pyrowave_critical_fec_percentage`; clients without reports keep that configured protection. Record-capable clients protect the coarse picture. Legacy clients use standard whole-frame parity within their four-block limit; large frames may receive less protection and show a warning. Their parity uses some of the existing picture budget; set PyroWave error correction to 0 to retain the unprotected format. Every encoded frame is limited to one nominal frame budget, including the first frame after a pause.

**Leave AMF's low-latency mode and AV1 latency mode on Driver default.** In the [October 12, 2026 Rubylight 2.2.9 property sweep](../rust/PERFORMANCE.md#retained-amf-property-sweeps-october-12), default ultra-low-latency usage already read back internal low latency for H.264/HEVC and AV1 latency mode 3 on the RX 7900 XT. Explicitly forcing the same settings produced similar timings across three runs, not exact equality or a demonstrated speedup. With Low latency or Transcoding usage, single auto/forced pairs changed the readbacks from off to on and showed mean reductions of 0.30-0.31 ms for HEVC and 1.01-1.37 ms for AV1. These are eight-second synthetic 1440p120/20 Mbps component tests, not full-stream latency measurements. The [October 7 audit](../rust/PERFORMANCE.md#october-7-amf-low-latency-mode-and-av1-latency-mode) documents earlier measurements and the RX 9000 HEVC-freeze concern; forcing these settings remains opt-in.

**Adaptive quantization (`amd_vbaq`) is off for H.264 and on for HEVC and AV1 when unset.** In the [October 12, 2026 Rubylight 2.2.9 settings comparison](../rust/PERFORMANCE_WORK.md#current-amf-quality-settings-comparison), explicitly turning H.264 AQ on reduced VMAF in all four cases: by 1.44 and 11.89 points on the two clips at 1440p120/20 Mbps, and by 0.06 and 0.41 at 50 Mbps. Turning AQ off gave mixed HEVC results, while AV1 VMAF rose in all 14 tested cases by 0.0009-0.2301 points in SDR and 0.0051-0.4822 in converted HDR. Those small AV1 gains do not establish a universal visual-quality benefit; the HDR scores also use converted SDR footage. Set AQ to On or Off to compare your own content, or Driver default to leave it to the driver. The [October 8 findings](../rust/PERFORMANCE_WORK.md#amf-settings-per-codec-quality-at-a-given-bitrate) remain a separate historical result.

The [current default-quality results](../rust/PERFORMANCE_WORK.md#current-amf-default-quality) cover 32 cases; the [settings comparison](../rust/PERFORMANCE_WORK.md#current-amf-quality-settings-comparison) adds 116 alternatives for quality preset, rate control, AQ, usage and high-motion boost. Each case encoded and scored exactly 180 frames, with no minimum VMAF pass threshold. CBR increased p99 frame size in every tested case; its score changes depended on codec and content. Ten high-motion-boost requests completed but lacked effective-setting readback, so their application is unverified. Encode-time deltas use earlier baselines rather than randomized matched trials. HDR rows convert SDR footage with 203-nit white and do not establish native-HDR perceptual quality.

**Leave split-frame encoding on Automatic.** AMF can split one HEVC or AV1 frame across a Radeon's two encoder engines (`amd_split_frame`), and the driver decides whether it does. Automatic asks for it only when the GPU has two engines and the driver has it off; On asks whenever there are two engines, and Off disables the property. In the [October 12, 2026 Rubylight 2.2.9 sweep](../rust/PERFORMANCE.md#retained-amf-property-sweeps-october-12), RX 7900 XT readbacks were 1 for Automatic/On and 0 for Off. Identical-input synthetic throughput was similar: for example, two-run means for 4K HEVC were 197.32 / 197.37 / 197.36 FPS respectively. This does not mean every frame had identical timing or prove how the driver used its engines. The sweep completed 708/720 calls; twelve pre-analysis cases failed and remain documented. It did not assess decoded quality. H.264 has no split-frame property; single-engine GPUs get no property write. [Historical October 7 measurements](../rust/PERFORMANCE.md#october-7-amf-split-frame-encoding) remain separate.

<a id="encoder-focus"></a>**Encoder focus (`encoder_focus`) is off unless you choose it.** A scene cut on a frame with a focus map comes out about twice the size of the same cut without one, which is why it is a choice and not a default. `static` (Still areas) attaches an AMF ROI map that gives blocks which stayed still for a few frames, such as text and HUDs, more bits than moving ones; `centre` (Still areas and centre) also favors the middle of the screen and the cursor. The stream stays a normal H.264, HEVC or AV1 stream for every client. It runs only on codecs the startup encoder check verified on this GPU (the host log's `encoder focus check:` line); otherwise the stream shows the `encoder_focus` warning and focus stays off. There is no automatic value. App and device overrides work as for the other encoder keys. Design and measurement plan: [docs/encoder-focus-design.md](encoder-focus-design.md).

**AMF automatically sizes recovery frames to packet pacing.** In the **AMD AMF** group of **All settings → Streaming**, the controls below use the stream's encoder bitrate and frame rate. Peak bitrate and VBV remain at driver defaults; maximum frame size defaults to `auto` when unset. Existing explicit settings, including `0` and the previous `2` default when saved explicitly, are preserved. Reconnect after saving. Initial values appear in the `AMF encoder settings` log line; each recovery picture reports its submitted cap after runtime adjustments.

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

## Displays and RTSS

Under **All settings → Display & HDR**, select a physical display, or a virtual display for each device or one shared by all devices. **Exclusive** (`virtual_display_layout`, the default) switches other monitors off; choose **Extended** to keep the existing desktop active. The primary and isolated variants control where the virtual display sits in that desktop.

A Remote Monitor follows the layout while it streams: exclusive turns the other displays off and makes it primary, the primary layouts make it primary, and it keeps the place the remote monitor layout gives it. Choose Extended to use a remote monitor beside your own. A layout you pick for a device under Devices overrides the host setting, and the host never replaces it. A device without one of its own keeps the layout you leave a stream's display in, such as Windows' "Show only on 2", and uses it from the next stream; set it back to the host setting to forget that. A layout changed through the clients update API (`/api/clients/update`) counts as set by you; send `virtual_display_layout_set_by_user: true` to mark one without changing it.

**Windows HDR Calibration on virtual displays** (`virtual_display_hdr_calibration`, default `false`) gives an HDR stream's new virtual display the profile the Windows HDR Calibration app made for your HDR monitor (the primary one first), and gives the display its own profile back after the stream. It is off by default because the profile describes that monitor, not the device you stream to. A profile chosen for the device (`hdr_profile`) comes first; with none found, the host log says so and the stream stays uncalibrated.

Resolution and refresh policies default to `auto` (`dd_resolution_option`, `dd_refresh_rate_option`). Virtual refresh follows the frame-limiting policy unless a manual or device display mode overrides it. To leave a **physical** monitor's resolution, refresh and HDR unchanged, select Disabled for `dd_configuration_option`.

A device's **Display mode** (`display_mode`, `WIDTHxHEIGHTxREFRESH`) sets its virtual display's resolution and refresh while the stream keeps the size and frame rate the client asks for, so a game can render at 1440p on a 4K stream. To pick just the resolution from Windows instead, leave the display mode empty and turn on **Devices → (device) → Remember the resolution I pick** (`remember_display_mode`): change the stream's virtual display resolution in Windows display settings during a stream, and when the device's last stream disconnects Rubylight keeps it (`remembered_resolution`), puts it back if the stream resumes, and creates the display at it from the next stream ([#36](https://github.com/RamazanKara/Rubylight/issues/36)). The Windows display scale you set on that display during the stream is kept the same way (`remembered_scale`, in percent, for example 200) and applied after the display is at its mode, so a 1440p display comes back at 200% too. A **Virtual display scale** set in the host settings (`dd_virtual_display_scale`, a percentage or Recommended) wins over a kept scale, and a scale the device asks for at launch (`rlDisplayScale`, Nightjar's **PC display scaling**) wins over both: then that scale is applied and no scale is kept. A scale the device asks for at launch that is too large for the display's resolution is lowered to the largest Windows offers there, with a warning, and one Windows refuses is a warning too: the stream goes on at the scale Windows shows. The device's **Display scale** set in the console is applied as set. A scale Windows refuses is logged as a warning and the stream goes on at the scale Windows shows, and only a scale Windows actually shows is kept. What is kept is the scale the display shows at the end, and Windows derives it from the recommended scale for the resolution, so a resolution change on its own (a game going exclusive fullscreen, say) can also be kept as a changed scale. The scale is put back when a paused stream resumes, so a change made at the PC while it was paused is not kept. The refresh is not kept: the stream's rate always wins, so the refresh follows the stream and **Virtual display refresh** as usual, including VRR's 1000 Hz, and a refresh Windows refuses falls back at the kept resolution. It applies to a device's own virtual display, not a shared one. Nothing is kept from a stream whose resolution Windows did not take, since the display then shows Windows' choice rather than yours. Turning the option off forgets the kept resolution and scale.

**Rotate the display** (`dd_rotation_option`, default `disabled`) gives the streamed display the device's portrait or landscape shape. Set it in **All settings → Display & HDR** or for one device under **Devices → (device) → Display overrides**. **Match the device** (`auto`) makes a device's own virtual display in the stream's shape, so a phone streaming in portrait gets a portrait display; a virtual display is never turned, since a turned one would reach the encoder sideways. A monitor gets the stream's mode in the shape it shows, then turns to the stream's shape. When the device turns during the stream and asks for the turned size (Nightjar's Orientation → Rotate), the virtual display changes shape or the monitor turns, off the stream loop; a display that cannot follow logs a warning and the stream's picture is scaled to the new size. **Choose below** (`manual`) turns the display to **Rotation** (`dd_manual_rotation`: `0`, `90`, `180` or `270` degrees clockwise); a virtual display is made in the turned shape instead. A monitor is turned back when the stream ends, and after a crash when the host next starts. A monitor another stream is using keeps its rotation. A turned monitor needs Windows Graphics Capture: Desktop Duplication refuses it with an error. A device's own **Display scale** (`dd_virtual_display_scale` in its overrides) also scales a monitor and is put back the same way; the host-wide Virtual display scale sets only virtual displays.

A disconnect leaves the app running but restores its displays by default: `dd_config_revert_on_disconnect=true`, after `dd_config_revert_delay` (default `3000` ms). Exclusive layouts always use that delay, including profiles that explicitly turn restoration off, so the physical monitors do not stay disabled while nobody is streaming. Other layouts can still be kept for reconnection by turning restoration off; `dd_paused_virtual_display_timeout_secs` then controls their lifetime (default `7200` seconds; `0` keeps them until the app closes). Restoration waits for the last stream using a layout. If that layout disabled an original display, the original layout is restored even after a Win+P change; other layouts preserve later user changes. A failed restoration remains in the recovery journal and the next stream retains its original baseline. A device remembers eligible layout changes only at its last stream's disconnect; later local changes during the pause do not become its streaming default. A client that sends nothing for `ping_timeout` (default `10000` ms) ends its stream. **Help & diagnostics** provides display restoration and saved-baseline controls.

**Virtual display → Default (by Windows version)** leaves the choice to the host: one virtual display per device on Windows 11 23H2 or later, none on earlier Windows. **Activate a virtual display** (`dd_activate_virtual_display`) is shown while Virtual display is unset or Off, or while it is on. It gives streams a virtual display when the mode is off, and it also overrides a device's or app's own Off. For **Permanent virtual displays**, an empty field preserves the driver's current count; `0` explicitly removes permanent displays at the next host startup. Resetting this setting is therefore different from entering `0`.

Under **All settings → Streaming**, in the **Frame limiter** and **RTSS** groups:

- Keep **Limiter** on Automatic, or select RTSS. Set **RTSS folder** (`rtss_install_path`) only if detection fails; an empty value searches Program Files.
- Rubylight starts RTSS when a limit is needed and restores the previous limit after disconnect. If RTSS requires administrator access, the installed service can use the signed-in administrator's token; otherwise start RTSS with the required access yourself.
- **Frame limit** (`frame_limiter_fps_limit`) defaults to `0`, meaning the stream rate. RTSS preserves fractional rates such as 59.94 FPS.
- **Virtual display refresh** offers 2× (`legacy`), 4× (`enabled`), 1000 Hz (`vrr`) or Off (`disabled`). Turning **Limit every stream** off does not disable the automatic virtual-display limit. Choose **Limiter → None** (`frame_limiter_provider = none`) to disable all limiting.

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

## Host bitrate adaptation

**All settings → Streaming → Adapt bitrate on the host** enables
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
feedback and wire budget. The controller is validated with
simulated loss, traffic, stale feedback and ownership tests. Device/network
performance still needs live measurements; it does not promise lower latency
on every connection.

## Error correction chosen by the device

A device that supports it (Nightjar) chooses its own share of error-correction packets for each stream, a fixed percentage or automatic (`rlFecPercentage`), and reports its loss to the host. **Most error correction a device may ask for** (`fec_max_percentage`, default `50` %) is the most it may choose. Devices that choose nothing get the host's own adaptive error correction, as Moonlight clients always have. The automatic frame cap follows the share chosen for each frame, so a scene cut stays inside what that share can protect. On shaped Wi-Fi links the device's automatic share cut median loss from 2.37 to 0.26 % and freezes from 21 to 1 against the host's adaptive share ([October 27](../rust/PERFORMANCE.md#october-27-client-error-correction-and-a-link-test-before-the-first-bitrate-nuc)).

**Bandwidth test** (`bandwidth_probe`, default `true`) lets a paired device time a short download from the host (`GET /bandwidth-probe`) before it picks a bitrate. The device decides whether to run it; with the setting off the host refuses the download.

## Roaming

**Client roaming** (`client_roaming`, default `true`) lets a device whose address changes, such as a phone moving from Wi-Fi to mobile data, keep its stream. The device proves the new address with the stream key first; the host then sends there and drops what arrives from the old address. Only devices that support it (Nightjar) use it; other Moonlight clients stream as before. Off, the host neither offers it nor answers roaming datagrams. Turning it off applies at once; turning it on applies from the next stream.

## Game-only audio

Game-only audio uses the normal audio stream and works with every client.

**All settings → Audio & controls → Audio to stream** defaults to **System audio**, which keeps
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

Under **All settings → Audio & controls**:

- **Keyboard follows text fields** (`keyboard_follows_focus`, default `true`) opens the device's keyboard when a text field on the PC gets the focus and hides it after, on devices that support it. **Send the text around the cursor** (`text_input_surrounding_text`, default `false`) gives the device's keyboard the text around the cursor for its suggestions; it never sends text from password fields.
- **Keyboard layout** (`keyboard_layout`, default `client`) turns the device's keys into scan codes with the layout the device reports; `us` uses US English, or enter a Windows layout id.
- **Native pen and touch** (`native_pen_touch`, default `true`) passes pen and touch to Windows as pen and touch. **Touch** (`touch_mode`, default `client`) takes touches as the device sends them, as a mouse, or as a trackpad, with **Touch sensitivity** (`touch_sensitivity`, `100` %) and **Touch gestures** (`touch_gestures`, two-finger scroll and right-click, default `true`). **Pen barrel roll** (`pen_barrel_roll`, default `true`) passes on how far a stylus is turned, for pens that report it.
- **Device-drawn cursor** (`client_cursor`, default `true`) lets a device draw the mouse cursor itself, so it moves without waiting for the video. It needs Desktop Duplication capture; with other capture the cursor stays in the video.

## Controllers

**All settings → Audio & controls → Controller type** controls which virtual controller games see. Standard profiles use the VHF gamepad driver included in the installer. Composite DualSense waveform haptics and native Steam Deck controllers use the optional usbip-win2 driver, bundled in the installer as **Steam Deck and DualSense USB audio and haptics**.

`gamepad = auto` matches each client's controller. PlayStation clients get a DualSense, the only pad with adaptive triggers; it also carries motion, battery, touchpad, lightbar and rumble. A game's trigger effects reach the player only if the client forwards them to a real DualSense. Nintendo clients get a Switch Pro. Other clients get an Xbox Series pad, or a DualSense when they have motion sensors or a touchpad and `motion_as_ds4` or `touchpad_as_ds4` is enabled. Both preferences default to enabled, so a Steam Deck reporting motion gets a DualSense.

`vhf_xbox`, `vhf_xbox_one`, `vhf_ds4`, `vhf_ds5` and `vhf_switch` give every client that pad: `vhf_ds5` a DualSense, `vhf_ds4` a DualShock 4 without adaptive triggers. Logs name the profile for each connected controller.

Earlier releases also offered Xbox 360 (`x360`) and DualShock 4 (`ds4`) through ViGEmBus. Rubylight changes those settings to `vhf_xbox_one` and `vhf_ds4` when it loads them, in the host configuration, app and device overrides, and configurations imported from Sunshine, Apollo or Vibepollo. An installed ViGEmBus is no longer used and can be uninstalled.

### DualSense waveform haptics

With a waveform-capable client and usbip-win2 installed on the host, Automatic can expose a DualSense with its matching USB audio endpoint. **DualSense with waveform haptics (USB/IP)** (`gamepad = usbip_ds5`) explicitly requires this mode. Games can then send their native actuator waveforms alongside ordinary rumble and adaptive triggers. See [setup and current limitations](dualsense-haptics.md).

**Haptics from game audio** (`audio_haptics`, off by default) feeds such a client's DualSense waveform haptics made from the game's bass and impacts while the game sends it none, with `audio_haptics_strength` (0 to 200 %), `audio_haptics_curve` (`linear`, `soft`, `punchy`) and `audio_haptics_gate` (-80 to -10 dBFS). It needs no USB/IP and stops as soon as the game vibrates the controller itself. See [haptics from game audio](dualsense-haptics.md#haptics-from-game-audio).

### Steam Deck

A Steam Deck can reach the host as a **real Steam Deck controller**: Rubylight serves the Deck's own USB controller (Valve 28de:1205) over USB/IP on the host's loopback address, and [usbip-win2](https://github.com/vadimgrn/usbip-win2) attaches it with its signed driver, so Windows sees a Steam Deck plugged into a USB port. Steam on the host then recognises a Steam Deck and applies the Deck's Steam Input layout, with both trackpads, the gyro, the back grips (L4, R4, L5, R5), the Steam and "…" buttons and rumble. Games see whatever Steam Input makes of it, as on a Deck.

- **All settings → Audio & controls → Steam Deck and back grips → Steam Deck controller** (`steam_deck_controller`): **Automatic** (`auto`, the default) attaches a Steam Deck when usbip-win2 is installed and Steam is running on the host when the controller connects; **Steam Deck** (`steam_deck`) attaches one whenever usbip-win2 is installed, even without Steam, where only SDL-based games see it; **Virtual pad** (`virtual_pad`) always uses the virtual DualSense or Xbox pad below.
- **Install USB audio and haptics support on the host.** The installer bundles usbip-win2 0.9.8.1 as the optional **Install USB audio and haptics support** component; install it when reconnecting USB devices is convenient. Setup can reconnect USB hubs and require a restart. Rubylight finds `usbip.exe` through its registered installation, `C:\Program Files\USBip`, or `PATH`. Nothing listens beyond `127.0.0.1`.
- If usbip-win2 is missing or the attach fails, the Deck falls back to the virtual pad, and the stream card says why when **Steam Deck** was chosen or an attach failed.

Moonlight passes on the Deck's own controls only when Steam Input is off for Moonlight on the Deck. With it on, Steam on the Deck turns the controls into a virtual pad first: the host still gets a Steam Deck with the buttons, sticks and triggers, but no gyro, trackpads or back grips, because Moonlight never receives them. On the Deck, open Moonlight's controller settings in Steam, choose to disable Steam Input, and reconnect. If the paired device's name says Steam Deck (SteamOS calls it `steamdeck`) and its controller arrives without a gyro, the stream card says this.

Moonlight sends trackpad touches from builds whose SDL reads the Deck's trackpads (SDL 3).

With the virtual pad (no usbip-win2, or **Virtual pad**), Moonlight's Steam controller with gyro gets a DualSense:

- **Gyro and accelerometer** are the pad's motion sensors.
- **Trackpads** share the pad's one touch surface: the left trackpad is its left half and the right trackpad its right half. Clicking the right trackpad is the touchpad click.
- **Back grips** (L4, R4, L5, R5): the virtual pads have no back buttons, so each grip presses what **All settings → Audio & controls → Steam Deck and back grips** sets for it: a face button, bumper, a fully pulled trigger, a stick click, Back, Start, Guide, a d-pad direction, the touchpad click or Share/Mute (`back_grip_l4`, `back_grip_r4`, `back_grip_l5`, `back_grip_r5`; values `a`, `b`, `x`, `y`, `lb`, `rb`, `lt`, `rt`, `l3`, `r3`, `back`, `start`, `guide`, `dpad_up`, `dpad_down`, `dpad_left`, `dpad_right`, `touchpad`, `misc`). They default to nothing, and pressing an unmapped grip shows a hint on the stream card. The same settings map an Xbox Elite's paddles (P1 is R4, P3 is L4, P2 is R5, P4 is L5) and a DualSense Edge's back buttons.
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

The menu offers Back to game, Disconnect (the game keeps running), End session and Close game. Each runs with the device's permissions as the console has them then, the same checks as the native API's session actions; End session and Close game ask for a second press within 5 s. While the menu is open the game gets no input, and everything the pad, keyboard, mouse and touch held is released when it opens. B or Esc on the top page, the chord again, a tap outside the panel or Back to game close it. Nightjar and other native clients open and close it with `POST /api/native/v1/session/couch-menu` when the host lists `session.couch_menu`.

Input-only sessions and the read-back encode path (`wgc_direct_encoder_input = false`) have no picture the host draws on, so the menu stays closed there and the host log says why; so does a stream too small for the panel.

## Close apps on disconnect

**Settings → Games → Close the app when the last device disconnects** (`terminate_on_pause`, default `false`) closes the running app after its last streaming device leaves. When off, the app stays running so a device can reconnect. Input-only connections do not keep the app running. The setting takes effect the next time an app starts, including apps started before Windows sign-in.

Each app's **Behavior → Close the app when the last device disconnects** offers **Use global default**, **On** and **Off**. Existing explicit choices, including imported choices, still override the global setting; select **Use global default** to make such an app follow it again. Apps without a saved choice follow the global default, including newly added and automatically synced games.

Closing an app runs its normal shutdown and cleanup; unsaved progress may be lost. This setting controls what happens when a device disconnects. Ending the stream when a game exits remains governed by app process tracking, **Keep the stream when a launcher exits within 5 seconds** and **Wait for every process the app starts**.

## Playnite launch focus

When launching a Playnite game or fullscreen menu, Rubylight can bring its window to the front so controller input reaches it. Under **All settings → Games & automation → Playnite launch focus**, **Confirm game focus** (`playnite_focus_attempts`, default `3`) sets the number of successful focus confirmations. Rubylight checks once a second, for at most **Stop focusing after** (`playnite_focus_timeout_secs`, default `15` seconds).

If launch keeps interrupting a window you are using on the host, enable **Stop after the first focus confirmation** (`playnite_focus_exit_on_first`, default `false`) or set either numeric limit to `0` to disable focus management. Stopping early lets a launcher or splash screen take focus back; increase the timeout when a slow launch otherwise leaves the game behind another window. Save and launch again to apply these settings.

## Radeon settings per game

An app can ask for Radeon driver settings while it runs. On the app's page in **Games**, the **Radeon** panel picks a profile:

| Profile | Writes |
|---|---|
| Competitive | Radeon Anti-Lag on |
| Smoothness | Frame Rate Target Control on, at the stream's frame rate |
| Quality | Anisotropic filtering on and the driver's "enhance application settings" anti-aliasing |
| Custom | Up to 24 settings you list (ids as in `GET /api/native/v1/radeon`; a feature's `.enabled` toggle goes first) |

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
**Devices → (device) → Play in the background** for every stream of that
device (`background_stream`), or **Library → (app) → Behavior → Stream in the
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
  PC user's calls and music stay private. The game is still audible on the
  PC's speakers unless you route it to another playback device.

A game needs no handler from Nucleus Co-op's handler hub for this: without
one, the host writes its own one-player handler. The app's command must be the
game's .exe or a Steam link; a Steam game starts with its Steam app id, so it
doesn't restart through Steam. Other launchers' links, launchers' .exe files,
games with anti-cheat and games on a network share are refused, and some games
pause anyway when their window is not
in front ([Games without a handler](nucleus-coop.md#games-without-a-handler)).
Co-op still needs a hub handler. Nucleus Co-op needs the host running as the
Windows service and an administrator account signed in on the PC;
[Nucleus Co-op](nucleus-coop.md) has the install, the limits, and co-op for
several devices at once. Two settings under
**All settings → General → Background streaming** shape it:
**Nucleus Co-op folder** (`nucleus_path`, empty: the host's own copy, which it
downloads on first use) and **Co-op players** (`nucleus_coop_players`, default
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

## Settings for one app or device

Open an app in **Games** or a paired device in **Devices** to set its display, HDR and other overrides. Leave an override unset to inherit the host configuration.

For general configuration overrides, Rubylight applies **host settings → device overrides → app overrides → the app's overrides for that device**. Only supported stream, input, display and encoder keys are accepted; host-wide network, identity and path settings cannot be overridden per stream.

Display selection has dedicated rules: a device's explicit virtual-display mode takes priority over the app's mode, and its **display mode** (`WIDTHxHEIGHTxREFRESH`) overrides the host's resolution/refresh policy. This does not change the frame rate requested for the encoded stream.

### One game on one device

An app can have settings of its own for one paired device, such as a 60 fps frame limit on the TV and 120 on a handheld. On the app's page in **Games**, the **Per device** panel adds a paired device and sets its **Stream preset**, **Frame limit** (`frame_limiter_fps_limit`, 0 for the stream's frame rate), **Radeon preset** and [managed game files](#managed-game-files). "Same as the game" leaves the app's own setting in place.

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

**Host control** (`host_control`) lets one paired device close games and change Radeon settings. It is not part of the other permissions. Open **Devices**, open the device, then under **Permissions → Custom permissions → Other** tick **Host control: change Radeon settings and close games from this device** and save.

- It is off for every device, including new pairings and devices imported from Apollo or Vibepollo. Only the console turns it on; nothing on the streaming port can.
- Turning it off applies on the device's next request. It does not end a running stream.
- A device also needs the **List apps**, **View streams** and **Launch apps** permissions for what it does. Reading the Radeon state needs View or Launch only.

### Sleep PC and sleep after idle

**Sleep PC** (`allow_sleep`, off for every device) is set in **Devices → device → Custom permissions → Other**. It lets that device put this PC to sleep through Vibepollo's `/power-actions/v1` requests while nothing is streaming or running: a single-use token valid for 30 s, then the suspend once the answer is sent and the connection closed. It needs a PC with S1, S2 or S3 sleep, as most desktops have. Windows has no call to enter Modern Standby, so on a PC with only Modern Standby the host advertises no Sleep PC, refuses the request, and the console shows why instead of the setting; the host log says so at startup and names the sleep states the PC reports. The console grants it as `true`; a device imported from Vibepollo with `allow_sleep: true` keeps it.

**Sleep after idle** (`sleep_after_idle_minutes`, under **All settings → General → Power**, default `0`, off) puts the PC to sleep when nobody has streamed or used it for that many minutes, 5 to 1440. **Sleep with a suspended stream** (`sleep_while_suspended`, default `false`) also lets it sleep while a stream is suspended or an app launched from a device still runs. A paired device can wake the PC again with Wake-on-LAN; [Privacy](privacy.md#your-own-devices) lists what it reads for that.

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

## Clipboard, files and USB

Each of these needs a permission you give a device on **Devices**; pairing never grants them, and **Full control** leaves the file, folder and USB ones as they are.

- **Clipboard.** A device with the clipboard permissions copies text to and from this PC, and images too while **Copy images** (`clipboard_image`, default `true`) is on, up to **Largest image** (`clipboard_image_max_mb`, `16` MiB as PNG) in each direction. Background streams have no clipboard.
- **Files and shared folders.** See [File transfer](#file-transfer) and [Shared folders](#shared-folders) below.
- **USB devices.** **USB forwarding** (`usb_forwarding`, default `false`, under **All settings → General → USB devices**) lets devices with **USB devices** attach the USB devices plugged into them to this PC during a stream, up to **USB devices at once** (`usb_max_devices`, default `4`). It needs usbip-win2 on the host; [USB forwarding](usb-forwarding.md) has the setup.

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

None by default. **Shared folders** (`shared_folders`, under **All settings → Network & access**) lists folders of this PC that paired devices may browse, download from and upload to over the streaming HTTPS port ([Native API, Shared folders](native-api.md#shared-folders)). **Add folder** adds a row with a name, which devices show, and a path, typed or picked with **Browse**. The path must be a folder on a drive of this PC, such as `D:\Games`: a network share, a relative path, `.` or `..` is refused. The console shows what is wrong beside the field (no name, two folders with one name, a path the host refuses) and saves nothing until it is fixed.

Each folder gets an id when it is first saved, from its name and four random letters, such as `games-k2fq`; devices' grants name it. Renaming a folder or changing its path keeps the id, and so what devices may do with it; a folder removed and added again gets a new id, so it is shared with no device until you share it again. The setting is stored as a JSON list such as `[{"id": "games", "name": "Games", "path": "D:\\Games"}]`, where `id` is 1 to 64 letters, digits, `-` or `_`; to edit it by hand, keep each id. A list that isn't valid is reported in the log and shares nothing, and the console says it can't read it.

A device sees a folder only once you share it with that device: on **Devices**, open the device's **Settings**, then **Shared folders**, and choose **Read** (browse and download) or **Read and write** (also upload) for each folder. **Full control** and the file transfer permissions don't share any folder. A grant for a folder no longer shared is listed as no longer shared, and **No access** removes it. Taking a folder away from a device, or write access to it, disconnects it and ends its running transfers. File transfer doesn't need to be on, but its size limit and speed while streaming apply. Files are read and written as the Windows user who is signed in, only inside each folder: links and junctions inside it are neither listed nor followed.

## WebRTC server

Off by default. With **WebRTC server** (`webrtc_server`) on, Rubylight listens for TVs and browsers that speak the Moonlight WebRTC protocol (version 2). They can find the PC, pair, sign in, list its apps and stream. [WebRTC server](webrtc.md) has the setup, what streams and the protocol extensions for client authors.

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

## Multiple instances

Give each instance its own profile folder and base port. An explicit `--config-dir` takes precedence over the portable defaults. From the extracted package, run these commands in separate PowerShell windows, one in each intended Windows session:

```powershell
.\butterpollo.exe --config-dir C:\RubylightProfiles\Seat1 --port 48123
.\butterpollo.exe --config-dir C:\RubylightProfiles\Seat2 --port 48323
```

The consoles are at **https://localhost:48124** and **https://localhost:48324**. These base ports also keep the instances' streaming ports separate from each other and from the default installed host. To make the port choice persist when using **Start Rubylight.exe**, save `port = 48123` or `port = 48323` in the corresponding profile's `sunshine.conf`. The launcher accepts `--config-dir`; `--port` is a host option.

For copied portable packages, create a separate `config` folder beside each copy of `butterpollo.exe` and set a different `port` in each folder's `sunshine.conf` before launching. Without an adjacent folder or explicit path, copies running as the same account share the per-user default, including SYSTEM's systemprofile location. Start with fresh profiles for independent identities and pair each instance separately; copying a populated profile also copies its identity and paired devices.

For multiseat use, launch each process in its intended Windows session through your multiseat software, using that session's SYSTEM token if required by your setup. The installer continues to manage one Windows service. Separate-profile startup has been smoke-tested with two simultaneous hosts.

## Updates

Automatic **checking** is on; automatic **installation** is not. **Help → Updates** checks on request and starts an offered update. **Include pre-releases** (**All settings → General → Updates**) also offers release candidates.

If you opt into **Install updates automatically**, the installed Windows service waits for one minute without active streams, pending connections, remote-monitor sessions or running host apps. The updater validates the selected installer against the release's size and SHA-256. Portable installations use the release download page instead.

Set `update_check_interval = 0` to disable scheduled checks; manual checks remain available.

## All settings

Every setting in the console, by the console's own groups, with its key and the default an unset key uses. Settings on the **Settings** home are in bold. Search the console for a key to find its field.

### General

#### General

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| **Host: Host name** | `sunshine_name` | empty | The name Moonlight shows for this host. Leave it empty to use the PC's name; local network discovery picks up a change after a restart. |
| Host: Fallback console language | `locale` | `en` (English) | Language used by the fallback console and reported to connected tools. This console is currently available in English only. |
| **Host: Allow pairing** | `enable_pairing` | `true` | Let new devices pair with this host. Turn it off once your devices are paired; devices already paired keep working. |
| Tray: Tray icon | `system_tray` | `true` | Show a Rubylight icon in the Windows notification area, with a shortcut to this console. |
| Tray: Hide tray controls | `hide_tray_controls` | `false` | Remove the disconnect, restart and quit actions from the tray icon's menu, so people at the PC cannot stop the host from there. |
| Updates: Update check interval | `update_check_interval` | `86400` s | How often the host looks for a new Rubylight release. Set it to 0 to turn off automatic checks. |
| **Updates: Install updates automatically** | `auto_update` | `false` | Download and install new releases on their own while nothing is streaming. |
| Updates: Include pre-releases | `notify_pre_releases` | `false` | Also offer pre-release versions when checking for updates. They get fixes sooner but may have more bugs. |
| Power: Sleep after idle | `sleep_after_idle_minutes` | `0` min | Put this PC to sleep when nobody has streamed or used it for this many minutes. 0 turns it off. |
| Power: Sleep with a suspended stream | `sleep_while_suspended` | `false` | Let the PC sleep after idle while a stream is suspended or an app launched from a device is still running. |
| Logging: Log level | `min_log_level` | `info` (Information) | How much detail the host writes to its log; changes apply right away. Use Debug when you are preparing a problem report. |
| Hardware metrics: Send hardware metrics to client | `hardware_metrics` | `false` | Share supported CPU and GPU readings when a compatible client requests them during a stream. Off by default; unavailable readings are omitted. |
| Hardware metrics: Hardware metrics interval | `hardware_metrics_interval` | `1000` ms | Minimum time between samples requested by a streaming client, from 500 to 10000 milliseconds. |
| Hardware metrics: Hardware metrics selection | `hardware_metrics_selection` | none | Which readings to send. Leave it empty to send every supported reading. |
| Logging: Log file | `log_path` | `logs/butterpollo.log` | Where the host writes its log. A relative path is inside the host's configuration folder. |
| Compatibility: App order for older devices | `legacy_ordering` | `false` | Add invisible characters to app names so older versions of Moonlight list apps in your order. This can break scripts and tools that match app names exactly. |
| Compatibility: Whole-number frame rate for commands | `envvar_compatibility_mode` | `false` | Give commands SUNSHINE_CLIENT_FPS as a whole number, such as 60 instead of 59.940. Turn it on if a script cannot read fractional frame rates. |
| USB devices: USB forwarding | `usb_forwarding` | `false` | Let the devices you allow attach their USB devices to this PC during a stream. Each device also needs Allow USB devices. |
| USB devices: USB devices at once | `usb_max_devices` | `4` | How many USB devices this PC takes from devices at the same time. |
| Clipboard: Copy images | `clipboard_image` | `true` | Let devices with clipboard permission copy images to and from this PC, besides text. |
| Clipboard: Largest image | `clipboard_image_max_mb` | `16` MiB | Images larger than this, as PNG, are refused in both directions. |
| Background streaming: Nucleus Co-op folder | `nucleus_path` | empty | The folder of your own Nucleus Co-op copy. Empty: the host's own copy, which it downloads on first use. |
| Background streaming: Co-op players | `nucleus_coop_players` | `4` | The most players a Nucleus co-op session gets. The game's handler and the free controller slots can allow fewer. |

#### Advanced

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Debug switches: Lower the bitrate when the link cannot carry it | `link_rate_adaptation` | `true` | Always on; listed under Debug switches for troubleshooting only. Cuts the picture bitrate when the network loses more than error correction can repair, then probes back up. Does nothing on a healthy link. Acts on three unrecoverable frames in two seconds, 5% of packets reported missing for two half-second windows, three frames lost on send, or three network keyframe requests from a client that sends no error-correction status. Each cut aims at 85% of what the link delivered, then the host probes back up in small steps and returns to the requested bitrate after ten clean seconds. Reports for frames sent before a cut took effect are ignored. Off for PyroWave. Adapt bitrate on the host wins when both are on, and a device that sets its own bitrate takes over bitrate control. |
| Displays: Default stream mode | `fallback_mode` | `1920x1080x60` | The resolution and frame rate used when a launch request names none, as WIDTHxHEIGHTxFPS. Devices normally send their own. |
| Debug switches: Pair with a QR code | `qr_pairing` | `true` | Always on; listed under Debug switches for troubleshooting only. Shows a QR code on Home and the Devices page that Nightjar scans to pair, instead of typing a PIN. The code holds a one-time PIN and passphrase that work once and expire after 3 minutes, and the fingerprint of the host certificate, which Nightjar checks before pairing. |

### Streaming

#### Video

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| **Preset: Stream preset** | `stream_preset` | `balanced` (Balanced) | Fills in the AMD encoder settings you have not set yourself. Settings you change below always win. |
| Capture: Capture method | `capture` | `auto` (Automatic) | Automatic uses direct driver push on Rubylight virtual displays and Windows Graphics Capture on other displays. |
| Capture troubleshooting: Capture poll interval | `capture_poll_interval_us` | `500` µs | How long screen capture waits before checking again when no new frame is ready. Shorter waits notice new frames sooner and use more CPU time. |
| Capture troubleshooting: GPU fence transport | `idd_push_fence_ring` | `true` | On by default. Turn it off to use the keyed-mutex ring for frames from the Rubylight display driver. |
| Capture troubleshooting: Predictive polling | `capture_predictive_poll` | `false` | With Desktop Duplication, checks for frames more often when the next one is due. Can catch frames sooner in games with a steady frame rate. |
| Capture troubleshooting: Align capture with encoding | `wgc_slot_aligned_publish` | `false` | Holds a new frame until just before the next encode if a newer one could arrive first. Helps when the display refreshes much faster than the stream. |
| Capture troubleshooting: Encode from GPU memory | `wgc_direct_encoder_input` | `true` | Hands captured frames straight to the encoder on the GPU. Turn off only to troubleshoot an encoder. |
| Capture troubleshooting: Copy and convert on a compute queue | `gpu_compute_conversion` | `true` | On supported AMD GPUs, copies captured frames and converts their colors on compute queues beside a game. Turn off to compare or troubleshoot. |
| Codecs: Full chroma | `experimental_packed_444` | `false` | Allow compatible Nightjar clients to request full-resolution color through HEVC or AV1 on AMD. Requires the client option. |
| Codecs: HEVC support | `hevc_mode` | `0` (Automatic) | Which HEVC profiles devices may request. A profile the encoder fails when the host starts is never offered. |
| Codecs: AV1 support | `av1_mode` | `0` (Automatic) | Which AV1 profiles devices may request. A profile the encoder fails when the host starts is never offered. |
| Codecs: PyroWave | `pyrowave` | `true` | Lets compatible devices request PyroWave, a very low-latency GPU codec. It needs hundreds of Mbps and a fast wired network. |
| Codecs: Prefer 10-bit SDR | `prefer_sdr_10bit` | `false` | When a device asks for HDR, keeps the display in standard dynamic range (SDR) and sends 10-bit SDR video. Reduces banding without switching to HDR. |
| Codecs: HDR dynamic metadata | `hdr_dynamic_metadata` | `true` | Adds HDR10+ brightness statistics to HDR video so compatible displays can tone-map scene by scene. Devices without HDR10+ ignore them. |
| Codecs: Dolby Vision | `hdr_dolby_vision` | `true` | Offers Dolby Vision profile 8.1, and profile 8.4 while HLG is on, on HEVC HDR streams and profile 10.1 on AV1 HDR streams, to devices that ask for it. |
| Codecs: HLG | `hdr_hlg` | `true` | Offers HLG HDR to devices that ask for it on HEVC and AV1 HDR streams. |
| Frame pacing: Frame pacing | `frame_pacing` | `arrival` (On arrival) | On arrival encodes each new frame as soon as it is captured. Fixed grid encodes the newest frame at fixed intervals. |
| Frame pacing: Steady grid timing | `wgc_pacing_smoothing` | `true` | Keeps the fixed grid on a steady schedule. When off, each interval starts from the previous encode, so one late frame delays the ones after it. |
| Frame pacing: Still-picture resend rate | `minimum_fps_target` | `20` fps | How often an unchanged picture is sent again. Lower values save bandwidth on a still desktop; it does not limit a game’s frame rate. |
| Frame pacing: Use the launch frame rate | `limit_framerate` | `false` | Encodes at the frame rate the device asked for when it launched the app instead of the faster rate its stream request may ask for. |
| Frame pacing: Follow client size and frame rate changes | `stream_reconfigure` | `true` | Lets a Rubylight client change the stream resolution and frame rate without reconnecting, for example when a foldable phone opens its inner screen. |
| Bitrate and network: Maximum bitrate | `max_bitrate` | `0` Kbps | Caps the bitrate a device can request. 0 uses the device's bitrate. |
| Bitrate and network: Error correction | `fec_percentage` | `20` % | Recovery packets sent with each video frame. Higher values survive more packet loss but leave less of the bitrate for the picture. |
| Bitrate and network: Adapt bitrate on the host | `host_adaptive_bitrate` | `false` | Lowers the bitrate when a Moonlight device reports video loss, then slowly raises it again while traffic is sustained and latency is stable. |
| Bitrate and network: Adapt error correction | `adaptive_fec` | `true` | Uses less error correction while the device reports a clean link, and returns to the percentage above as soon as it reports loss. |
| Bitrate and network: Most error correction a device may ask for | `fec_max_percentage` | `50` % | The largest share of recovery packets a device may choose, as a percentage of the video packets. |
| Bitrate and network: PyroWave error correction | `pyrowave_critical_fec_percentage` | `20` % | Recovery packets that protect PyroWave pictures against packet loss. |
| Bitrate and network: Send batch size | `video_max_batch_size_kb` | `64` (64 KiB) | Largest burst of video packets sent at once. Smaller batches can help devices that drop bursts. |
| Bitrate and network: Pacing rate | `pacing_max_bitrate_kbps` | `0` Kbps | Spaces out video packets without changing the picture bitrate. 0 paces automatically; reconnect after saving. |
| Bitrate and network: Adaptive packet pacing | `host_adaptive_pacing` | `false` | Starts with slower packet delivery, then tries faster rates and backs off when the device reports loss. |
| Bitrate and network: Adaptive packet pacing for Wi-Fi clients | `wireless_adaptive_pacing` | `true` | When a client says its connection is Wi-Fi or cellular, paces its video adaptively from the first frame instead of after the first lost frames. |
| Bitrate and network: Slow down after video loss | `keyframe_loss_pacing` | `true` | After a lost keyframe or repeated unrecoverable frames, spaces video packets more evenly for the rest of the stream without changing picture bitrate. |

#### Encoders

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Encoder: Encoder | `encoder` | `auto` (Automatic) | Automatic encodes H.264, HEVC and AV1 with AMD AMF, and PyroWave streams with PyroWave's own encoder. |
| Encoder: Graphics card | `adapter_name` | empty | The graphics processing unit (GPU) that captures and compresses the video. Automatic uses the card connected to the streamed display. |
| Encoder: Graphics card device ID | `adapter_pnp_id` | empty | Chooses the GPU by its Windows device instance ID instead of its name, for systems with two identical GPUs. When set, it takes precedence over the GPU name. |
| AMD AMF: Usage | `amd_usage` | `ultralowlatency` (Ultra low latency) | The base encoding profile; the settings below override parts of it. Profiles other than the low-latency ones add delay. |
| AMD AMF: Quality preset | `amd_quality` | `speed` (Speed) | Trades encoding speed for picture quality. Driver default lets the usage profile decide. |
| AMD AMF: Rate control | `amd_rc` | `vbr_latency` (Latency-constrained VBR) | How the encoder keeps to the stream bitrate. Constant QP ignores the bitrate. |
| AMD AMF: Peak bitrate | `amd_peak_bitrate_ratio` | `0` (Driver default) | Peak bitrate as a multiple of the requested stream bitrate. 0 keeps the driver default. |
| AMD AMF: Rate-control buffer | `amd_vbv_buffer_frames` | `0` (Driver default) | Rate-control buffer budget, in frames. Smaller budgets can reduce bursts at the cost of picture quality. |
| AMD AMF: Maximum frame size | `amd_max_frame_size` | `auto` (Automatic — follow network pace) | Requests a size limit for every frame, including recovery keyframes. Tight limits can reduce picture quality. |
| AMD AMF: Quality VBR level | `amd_qvbr_quality_level` | `0` | Target quality for quality VBR, from 1 (lowest) to 51 (highest). 0 keeps the driver default. |
| AMD AMF: Pre-analysis | `amd_preanalysis` | `false` | Analyzes each frame before encoding to improve rate control, at the cost of extra latency. Not used for HDR streams. |
| AMD AMF: Variance-based adaptive quantization | `amd_vbaq` | empty (Default (H.264 off; HEVC/AV1 on)) | Gives more bits to smooth areas, where artifacts are easiest to see, and fewer to busy textures. |
| AMD AMF: Enforce decoder buffer limits | `amd_enforce_hrd` | `false` | Holds rate control to the decoder buffer model, which greatly reduces bitrate spikes. Can cause artifacts or lower quality on some GPUs. |
| AMD AMF: H.264 entropy coding | `amd_coder` | `auto` (Driver default) | CABAC gives better quality per bit; CAVLC is simpler to decode. H.264 only. |
| AMD AMF: Input queue size | `amd_input_queue_size` | `0` frames | How many frames may wait for the encoder. 0 uses the driver default, VRR streams included. |
| AMD AMF: Long-term reference frames | `amd_ltr_frames` | `4` frames | Frames the encoder keeps as long-term references, so a client that lost packets recovers with a small frame instead of a full keyframe. H.264, HEVC and AV1. |
| AMD AMF: Low-latency mode | `amd_lowlatency_mode` | `auto` (Driver default) | Turns the encoder's internal low-latency mode on or off for H.264 and HEVC. |
| AMD AMF: High-motion quality boost | `amd_high_motion_quality_boost` | `auto` (Driver default) | Improves quality in fast motion on drivers that support it. |
| AMD AMF: SmartAccess Video | `amd_smart_access_video` | `auto` (Driver default) | Lets the driver share encoding between AMD integrated and discrete graphics on systems that support SmartAccess Video. |
| AMD AMF: Split-frame encoding | `amd_split_frame` | `auto` (Automatic) | Lets the driver split HEVC or AV1 frames across two encoder engines. Leave Automatic unless comparing your own GPU. |
| AMD AMF: Encoder focus | `encoder_focus` | `off` (Off) | Gives more bits to parts of the picture that stay still, such as text and HUDs, and fewer to motion. |
| AMD AMF: AV1 latency mode | `amd_av1_latency_mode` | `auto` (Driver default) | How quickly each AV1 frame is finished. Lower latency uses more power. |
| AMD AMF: AV1 screen content tools | `amd_av1_screen_content` | `auto` (Driver default) | AV1 coding tools for desktop content, which can make text and interface edges clearer. AV1 only. |
| AMD AMF: AV1 tiles | `amd_av1_tiles` | `0` (Automatic) | Tiles per AV1 frame. Automatic follows the device's slice request, up to 4. |

#### Frame limiting

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Frame limiter: Limit every stream | `frame_limiter_enable` | `false` | Caps the game’s frame rate on every stream, not only on virtual displays. Virtual-display streams are already limited unless Virtual display refresh is off. |
| Frame limiter: Limiter | `frame_limiter_provider` | `auto` (Automatic) | What enforces the limit. Automatic and RTSS use RivaTuner Statistics Server (RTSS), which must be installed; None turns off all limiting. |
| Frame limiter: Frame limit | `frame_limiter_fps_limit` | `0` fps | The frame rate games are capped at; 0 uses the stream’s frame rate. RTSS keeps decimals such as 59.94. |
| Frame limiter: Virtual display refresh | `frame_limiter_auto_virtual_framegen` | `legacy` (Twice the stream rate) | How fast a virtual display refreshes. A faster display captures each game frame sooner after it is drawn. |
| RTSS: RTSS folder | `rtss_install_path` | empty | Where RivaTuner Statistics Server is installed. Empty looks for it in Program Files. |
| RTSS: RTSS limit mode | `rtss_frame_limit_type` | `async` (Asynchronous) | How RTSS paces frames. Some streams use front edge sync instead. |
| RTSS: Use this mode on virtual displays | `rtss_allow_virtual_display_override` | `false` | Applies the RTSS limit mode to virtual-display streams instead of front edge sync. |

### Display & HDR

#### Display

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Streamed display: Display | `output_name` | empty | The monitor to stream when no virtual display is used. Empty streams the primary display; a device or app can choose its own. For testing (advanced, config file only): `window:<hwnd>/<pid>@<display>` streams that one window with WGC (the release harness's demo stream); `<pid>` is the process that owns the window and `<display>` the display the window is on (such as `\\.\DISPLAY1`). A malformed target, a closed or minimised window, a handle another process now owns, or a window on another display fails the stream with the reason, never falling back to a display. |
| **Streamed display: Virtual display** | `virtual_display_mode` | empty (Default (by Windows version)) | Whether streams use a virtual display sized for the device instead of a monitor. A device’s or app’s own choice wins. |
| **Streamed display: Virtual display layout** | `virtual_display_layout` | `exclusive` (Exclusive) | How the virtual display fits with your other monitors while streaming. Exclusive turns the other monitors off. |
| Streamed display: Virtual display scale | `dd_virtual_display_scale` | `0` (Keep) | The Windows scale set on a virtual display. Recommended picks one from the resolution, such as 125% at 1080p; Keep leaves the scale Windows remembers for that display. |
| Streamed display: Activate a virtual display | `dd_activate_virtual_display` | `false` | Streams use a virtual display even when Virtual display is off, unless the device asks for none. It also overrides a device’s or app’s own Off. |
| Streamed display: Monitor setup | `dd_configuration_option` | `verify_only` (Only check that it’s on) | What happens to the streamed monitor when the stream does not use a virtual display. Leaving the monitors alone also skips resolution, refresh rate and HDR changes on it. |
| Resolution and refresh rate: Resolution | `dd_resolution_option` | `auto` (Match the device) | The resolution set on the streamed display. Keeping the current one applies to monitors only; a virtual display matches the device unless a fixed resolution is set. |
| Resolution and refresh rate: Fixed resolution | `dd_manual_resolution` | empty | WIDTHxHEIGHT, for example 2560x1440. The width can be 320 to 7680 and the height 200 to 4320. |
| Resolution and refresh rate: Refresh rate | `dd_refresh_rate_option` | `auto` (Match the stream) | The refresh rate set on the streamed display. |
| Resolution and refresh rate: Fixed refresh rate | `dd_manual_refresh_rate` | empty | In hertz, with up to three decimals such as 59.94 or 119.88. On a virtual display it replaces the faster automatic refresh. |
| Resolution and refresh rate: Mode substitutions | `dd_mode_remapping` | `{"mixed": [], "resolution_only": [], "refresh_rate_only": []}` | Changes a requested mode before it is applied. Each entry matches requested_resolution and requested_fps and sets final_resolution and final_refresh_rate. |
| After a stream: Restore displays on disconnect | `dd_config_revert_on_disconnect` | `true` | Restore the displays after a device disconnects, even when its app keeps running. |
| After a stream: Restore delay | `dd_config_revert_delay` | `3000` ms | How long to wait after a device disconnects before restoring the displays. |
| After a stream: Keep a disconnected display for | `dd_paused_virtual_display_timeout_secs` | `7200` s | How long a disconnected device’s non-exclusive display is kept while its app runs. 0 keeps it until the app closes. |
| After a stream: Keep monitors off while paused | `dd_hold_layout_while_paused` | `false` | With an exclusive layout, keep switching the monitors back off while a device is disconnected, until it returns or the restore delay ends. |
| After a stream: Restore the saved layout | `dd_always_restore_from_golden` | `false` | After a stream, return the displays to the layout saved under Maintenance instead of the layout from before the stream. |
| After a stream: Displays the saved layout skips | `dd_snapshot_exclude_devices` | none | A list of display device IDs in JSON format (text enclosed in square brackets) that restoring the saved layout leaves as they are, such as a dummy plug. |
| After a stream: Restore hotkey | `dd_snapshot_restore_hotkey` | empty | A key on this PC that ends every stream and restores the display layout, for when a stream leaves the screens unusable. F1 to F24, a letter or a digit. |
| After a stream: Restore hotkey modifiers | `dd_snapshot_restore_hotkey_modifiers` | `ctrl+alt+shift` | Keys held with the restore hotkey: ctrl, alt, shift and win, joined with +. |
| Remote monitors: Mute remote monitors | `remote_monitor_mute_audio` | `false` | Sends picture and input, but no audio, to a device used as a remote monitor. |
| Remote monitors: Remove when its stream ends | `remote_monitor_disconnect_on_stream_end` | `false` | Removes a device’s remote monitor when its stream ends. When off, the monitor stays so the device can resume. |
| Remote monitors: Remove when the device disconnects | `remote_monitor_disconnect_on_client_disconnect` | `false` | Removes the remote monitor as soon as the device’s connection drops, even if it was kept for resuming. |
| Remote monitors: Quit the game on the first request | `remote_monitor_terminate_on_first_request` | `false` | Lets another device quit the running game with its first request. When off, it must ask again within 60 seconds; the device that started the game is never asked twice. |
| Remote monitors: Confirm before replacing a running app | `remote_monitor_confirm_app_replacement` | `true` | Makes a device ask twice before replacing a running app with a different one, so Moonlight can warn first. |
| Streamed display: Rotate the display | `dd_rotation_option` | `disabled` (Leave as it is) | Whether the streamed display takes the device's portrait or landscape shape, and follows it when the device turns. |
| Streamed display: Rotation | `dd_manual_rotation` | `0` (0°) | The angle the streamed display turns to, clockwise; a virtual display is made in that shape. |

#### HDR

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Display: Display HDR | `dd_hdr_option` | `auto` (Match the stream) | Turns HDR on the streamed display on or off to match the stream. |
| Display: HDR request | `dd_hdr_request_override` | `auto` (Follow the device) | Overrides the device’s HDR request for the display, for devices or games that report it wrongly. The stream itself still follows the device. |
| Display: Vulkan HDR on virtual displays | `vulkan_hdr_layer` | `true` | Registers a Vulkan layer so Vulkan games can use HDR on a virtual display. Turn it off if Vulkan games crash. |
| Display: Windows HDR Calibration on virtual displays | `virtual_display_hdr_calibration` | `false` | Gives an HDR virtual display the profile Windows HDR Calibration made for your HDR monitor, and gives the display back its own profile after the stream. |
| Display: Peak brightness | `hdr_peak_brightness` | `1000` nits | The brightest the device’s screen can show, which a virtual display reports as its peak. |

### Audio & controls

#### Audio

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| **Sound: Stream audio** | `stream_audio` | `true` | Send the host's sound to the device. Turn it off when a stream is only used as an extra monitor and should stay silent. |
| Sound: Audio to stream | `audio_capture_mode` | `system` (System audio (selected playback device)) | Choose Game only to stream just the game, keeping browsers and notifications out. Reconnect to apply. |
| Sound: PCM and Dolby Digital audio | `audio_passthrough` | `true` | Offers uncompressed PCM, Dolby Digital and Dolby Digital Plus audio to devices that ask for it, for TVs and receivers that decode it themselves. Other devices keep Opus. |
| Devices: Audio device | `audio_sink` | empty | The audio output captured when the streaming device keeps sound playing on the host, or when no virtual speakers exist. Leave it empty to use the Windows default output. |
| Devices: Virtual speakers | `virtual_sink` | empty | The audio output made the Windows default during a stream, so sound goes to the streaming device instead of the PC's speakers. |
| Devices: Capture without switching | `audio_sink_capture_only` | `false` | Capture the audio device without changing the Windows default output. You then send the apps you want to hear to that device yourself. |
| Devices: Follow the default device | `auto_capture_sink` | `true` | Capture whichever device is the Windows default, even if it changes during a stream. Turn it off to stay on the device chosen when the stream started. |
| Virtual speakers: Keep virtual speakers as default | `keep_sink_default` | `true` | If another device becomes the Windows default during a stream, switch back to the virtual speakers. The device you picked is still restored when the stream ends. |
| Virtual speakers: Install Steam audio drivers | `install_steam_audio_drivers` | `true` | When Steam is installed, install its streaming speakers and microphone when a stream needs them. |
| **Microphone: Use the device microphone** | `stream_mic` | `true` | Let a streaming device that sends its microphone talk through the PC. Only clients with microphone support send one. |
| 7.1.4 surround | `audio_surround_714` | `true` | Offers 7.1.4 surround sound to devices that ask for it. Height channels stay silent unless the captured playback device has them. |

#### Input

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Devices: Keyboard | `keyboard` | `true` | Let devices type on the host. |
| Devices: Mouse | `mouse` | `true` | Let devices use the mouse on the host. Pen and touch input need this too. |
| **Devices: Controllers** | `controller` | `true` | Let devices use game controllers on the host. |
| Devices: Remote input entry | `enable_input_only_mode` | `false` | Add a Remote Input entry to every device's app list. It sends keyboard, mouse and controller input to the host without video, for example from a second device. |
| Keyboard: Send scan codes | `always_send_scancodes` | `true` | Send keys as scan codes, which some games need to see keyboard input at all. They follow Keyboard layout. |
| Keyboard: Right Alt as Windows key | `key_rightalt_to_key_win` | `false` | Treat the right Alt key as the Windows key, for devices that cannot send the Windows key. |
| Keyboard: Key repeat delay | `key_repeat_delay` | `500` ms | How long a held key waits before it starts repeating. 0 turns key repeat off on the host. |
| Keyboard: Key repeat rate | `key_repeat_frequency` | `24.9` Hz | How many times per second a held key repeats. Decimals are allowed; 0 uses the default. |
| Keyboard: Key remapping | `keybindings` | none | Make a key from the device press a different key on the host, as pairs of Windows virtual-key codes such as ["0x10", "0xA0"]. |
| Keyboard: Keyboard follows text fields | `keyboard_follows_focus` | `true` | Show the device's keyboard when a text field on the host gets the focus, and hide it after. |
| Keyboard: Send the text around the cursor | `text_input_surrounding_text` | `false` | Send the text around the cursor to the device's keyboard for its suggestions, never from password fields. |
| Keyboard: Keyboard layout | `keyboard_layout` | `client` | The layout that turns the device's keys into scan codes: client for the one the device reports, us for US English, or a Windows layout id. |
| Mouse, pen and touch: High-resolution scrolling | `high_resolution_scrolling` | `true` | Pass fine scroll steps from the device. Turn it off for older apps that scroll too far, so scrolling moves in whole notches. |
| Mouse, pen and touch: Native pen and touch | `native_pen_touch` | `true` | Pass pen and touch from devices to Windows as pen and touch. Turn it off for apps that handle them badly; devices then send touch as mouse input. |
| Controllers: Controller type | `gamepad` | `auto` (Automatic) | The virtual controller the host uses. Automatic matches each device's controller; choosing a type here uses it for all connected controllers. |
| Controllers: PlayStation controller for motion controls | `motion_as_ds4` | `true` | With Automatic, motion sensors select DualSense. Turn it off to ignore motion sensors when choosing. |
| Controllers: PlayStation controller for touchpads | `touchpad_as_ds4` | `true` | With Automatic, a touchpad selects DualSense. Turn it off to ignore the touchpad when choosing. |
| Controllers: Vibration | `forward_rumble` | `true` | Send vibration from games back to the device's controller. |
| Controllers: Haptics from game audio | `audio_haptics` | `false` | A DualSense on the device feels the game's bass and impacts when the game sends it no vibration of its own. Changes apply during a stream. |
| Controllers: Audio haptics strength | `audio_haptics_strength` | `100` % | How strongly the bass drives the actuators. 0 is off; above 100 % quiet scenes are felt more but loud ones clip. |
| Controllers: Audio haptics response | `audio_haptics_curve` | `linear` (Linear) | Soft lifts quiet bass, Punchy keeps only the big hits, Linear follows the sound. |
| Controllers: Audio haptics noise gate | `audio_haptics_gate` | `-50` dBFS | Bass quieter than this is not felt, so music and ambience under it stay still. Raise it if the controller hums. |
| Controllers: Hold Back for Guide | `back_button_timeout` | `-1` ms | Holding Back (Select) this long presses the Guide (Home) button instead, for controllers without one. -1 turns this off. |
| Steam Deck and back grips: Steam Deck controller | `steam_deck_controller` | `auto` (Automatic) | How a Steam Deck's controls reach the host. Steam Deck lets Steam on the host see a real Steam Deck and apply its Steam Input layout. |
| Steam Deck and back grips: Back grip L4 | `back_grip_l4` | `none` (Nothing) | What the upper left back button presses: a Steam Deck's L4, or the matching paddle on an Xbox Elite or DualSense Edge. |
| Steam Deck and back grips: Back grip R4 | `back_grip_r4` | `none` (Nothing) | What the upper right back button presses: a Steam Deck's R4, or the matching paddle on an Xbox Elite or DualSense Edge. |
| Steam Deck and back grips: Back grip L5 | `back_grip_l5` | `none` (Nothing) | What the lower left back button presses: a Steam Deck's L5, or the matching paddle on an Xbox Elite or DualSense Edge. |
| Steam Deck and back grips: Back grip R5 | `back_grip_r5` | `none` (Nothing) | What the lower right back button presses: a Steam Deck's R5, or the matching paddle on an Xbox Elite or DualSense Edge. |
| Mouse, pen and touch: Pen barrel roll | `pen_barrel_roll` | `true` | Passes on how far a stylus is turned around its own axis, for pens and devices that report it. |
| Mouse, pen and touch: Touch | `touch_mode` | `client` (As the device sends them) | How this PC takes touches: as the device sends them, as a mouse or as a trackpad. Needs native pen and touch. |
| Touch sensitivity | `touch_sensitivity` | `100` % | How far the pointer moves for a swipe as a trackpad, and how far two fingers scroll. |
| Touch gestures | `touch_gestures` | `true` | As a mouse or trackpad, two fingers scroll and a two-finger tap clicks right. |
| Device-drawn cursor | `client_cursor` | `true` | Lets a device draw the mouse cursor itself, so the cursor moves without waiting for the video. Needs Desktop Duplication capture; with other capture the cursor stays in the video. |

### Games & automation

#### Library

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| **App behavior: Close the app when the last device disconnects** | `terminate_on_pause` | `false` | Closes the running app when its last streaming device disconnects. Off keeps it running for reconnection. |
| Steam options: Steam library | `steam_enabled` | `true` | Adds your Steam games to the library, with their covers, and ends a stream when the game it started exits. In Library, select Sync now beside Steam to add them now. |
| Steam options: Sync automatically | `steam_auto_sync` | `true` | Checks Steam every 30 seconds and updates the Steam apps when games are installed, removed or played. |
| Steam options: Every installed game | `steam_sync_all_installed` | `true` | Adds every installed game, as every launcher does. When off, only the games played most recently are added. |
| Steam options: Recent games | `steam_recent_games` | `10` | How many of the most recently played games to add. |
| Steam options: Played within | `steam_recent_max_age_days` | `30` days | Leaves out games not played for this many days. 0 adds them however long ago they were played. |
| Steam options: Remove uninstalled games | `steam_autosync_remove_uninstalled` | `true` | Removes a Steam app when its game is uninstalled. With recent games only, apps that fall out of the list are always removed. |
| Steam options: Include tools | `steam_include_tools` | `false` | Also adds tools, runtimes, redistributables and DLC that Steam lists as installed. |
| Steam options: Games to leave out | `steam_exclude_games` | none | A JSON list of games the sync never adds, by Steam app ID or name: ["570", {"name": "Dota 2"}]. Their Steam apps are removed. |
| Playnite: Playnite | `playnite_enabled` | `true` | Starts apps linked to a Playnite game through Playnite and ends the stream when the game closes. Needs the Rubylight plugin in Playnite. |
| Playnite: Add games automatically | `playnite_auto_sync` | `true` | While Playnite runs, adds the games chosen below as apps and removes them when they no longer qualify. When off, only apps already linked to a game are updated. |
| Playnite: Every installed game | `playnite_sync_all_installed` | `true` | Adds every installed game, as every launcher does. When off, the recently played games and the categories and plugins below are added. |
| Playnite: Recent games | `playnite_recent_games` | `10` | How many of the most recently played games to add. 0 adds none. |
| Playnite: Played within | `playnite_recent_max_age_days` | `30` days | Recent games must have been played within this many days. 0 counts any game, played or not. |
| Playnite: Categories to add | `playnite_sync_categories` | none | A JSON list of Playnite category names whose installed games are added: ["Couch", "Racing"]. |
| Playnite: Libraries to add | `playnite_sync_plugins` | none | A JSON list of Playnite library plugin IDs whose installed games are added. |
| Playnite: Games to leave out | `playnite_exclude_games` | none | A JSON list of Playnite game IDs the sync never adds. |
| Playnite: Categories to leave out | `playnite_exclude_categories` | none | A JSON list of category names whose games the sync never adds. |
| Playnite: Libraries to leave out | `playnite_exclude_plugins` | none | A JSON list of library plugin IDs whose games the sync never adds. |
| Playnite: Remove uninstalled games | `playnite_autosync_remove_uninstalled` | `true` | Removes an added app when its game is uninstalled. |
| Playnite: Remove unplayed after | `playnite_autosync_delete_after_days` | `14` days | Removes an added app not played within this many days of being added. 0 keeps it. |
| Playnite: Replace older recent games | `playnite_autosync_require_replacement` | `true` | When more recent games are played, removes as many older added games as are newly added, keeping the count. |
| Playnite: Playnite fullscreen app | `playnite_fullscreen_entry_enabled` | `false` | Adds a “Playnite (Fullscreen)” app that opens Playnite’s fullscreen mode, for choosing a game on the device. |
| Playnite launch focus: Confirm game focus | `playnite_focus_attempts` | `3` | How many times the game or Playnite fullscreen menu must be confirmed in front after launch. Lower it if Rubylight keeps interrupting another window. |
| Playnite launch focus: Stop focusing after | `playnite_focus_timeout_secs` | `15` s | Maximum time spent bringing a launched game or Playnite fullscreen menu to the front. 0 leaves focus alone, even when Confirm game focus is above 0. |
| Playnite launch focus: Stop after the first focus confirmation | `playnite_focus_exit_on_first` | `false` | Stop as soon as the game or Playnite menu is confirmed in front. Prevents later focus changes, but a launcher or splash screen may then take focus back. |
| Launchers: Epic Games | `library_epic_enabled` | `true` | Adds the games installed with the Epic Games Launcher to the library, with their covers, and ends a stream when the game exits. |
| Launchers: Battle.net | `library_battlenet_enabled` | `true` | Adds the games installed with Battle.net to the library, with their covers, and ends a stream when the game exits. |
| Launchers: EA app | `library_ea_enabled` | `true` | Adds the games installed with the EA app to the library, with their covers, and ends a stream when the game exits. |
| Launchers: Ubisoft Connect | `library_ubisoft_enabled` | `true` | Adds the games installed with Ubisoft Connect to the library, with their covers, and ends a stream when the game exits. |
| Launchers: GOG | `library_gog_enabled` | `true` | Adds the games installed with GOG Galaxy or the GOG installer to the library, with their covers, and ends a stream when the game exits. |
| Launchers: Xbox and Microsoft Store | `library_microsoft_enabled` | `true` | Adds the games installed with the Xbox app or the Microsoft Store to the library, with their covers, and ends a stream when the game exits. |
| Launchers: Sync automatically | `library_auto_sync` | `true` | Keeps every installed game of the launchers above in the library, like the Steam sync. Games you delete from the library stay deleted. |
| Launchers: Remove uninstalled games | `library_autosync_remove_uninstalled` | `true` | Removes a synced game once its launcher no longer lists it. Games you added by hand stay. |
| Game folders: Game folders | `library_local_folders` | none | Folders that hold one game per subfolder, such as D:\Games. Add games then lists each subfolder with the program that starts it. |
| Game folders: Folders to leave out | `library_local_exclude` | none | Game folders Add games never lists, by name (Tools) or full path (D:\Games\Tools). |
| Lossless Scaling: Lossless Scaling program | `lossless_scaling_path` | empty | LosslessScaling.exe or its folder, for apps that use Lossless Scaling. Empty finds it in Steam libraries and the usual folders. |
| Lossless Scaling: Let Lossless Scaling start by itself | `lossless_scaling_legacy_auto_detect` | `false` | Uses Lossless Scaling’s auto scale for the game instead of pressing its hotkey, and leaves its window open. An app can choose otherwise. |

#### Commands

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Around every app: Preparation commands | `global_prep_cmd` | none | Commands run before every app starts (do) and after it closes (undo). If a do command fails, the app does not start; apps can opt out in their settings. |
| Around every app: Pause and resume commands | `global_state_cmd` | none | Commands run when any app's stream resumes (do) and when it pauses (undo), before the app's own state commands. Apps can opt out in their settings. |
| From devices: Host commands | `server_cmd` | none | Commands a device can run on the host from its stream menu, in streaming apps that support it. Only devices with the Host commands permission see them. |

### Network & access

#### Network

| Setting | Key | Default | What it does |
| --- | --- | --- | --- |
| Ports and addresses: Base port | `port` | `47989` | Moonlight connects on this port, and the host's other ports are counted from it. Change it only if another program needs these ports. |
| Ports and addresses: IP versions | `address_family` | `ipv4` (IPv4 only) | Whether the host also accepts connections over IPv6. Ignored when a listen address is set. |
| Ports and addresses: Listen address | `bind_address` | empty | Listen on one of this PC's IP addresses instead of all of them; local network discovery then uses only that address. Leave it empty to listen on every address. |
| Discovery and internet: Local network discovery | `enable_discovery` | `true` | Announce this host on the local network so Moonlight finds it without typing an address. |
| Discovery and internet: UPnP port forwarding | `upnp` | `false` | Ask the router to forward the streaming ports so devices can connect over the internet. The router must have Universal Plug and Play (UPnP) turned on. |
| WebRTC: WebRTC server | `webrtc_server` | `false` | Let TVs and browsers that speak the Moonlight WebRTC protocol pair and stream. |
| WebRTC: WebRTC signalling port | `webrtc_port` | `8000` | The TCP port TVs connect their WebSocket to. Discovery always listens on UDP 8000, where TVs send their broadcast. |
| WebRTC: WebRTC media port | `webrtc_media_port` | `8001` | The UDP port for WebRTC video, audio and input. |
| File transfer: File transfer | `file_transfer` | `false` | Let paired devices send files to this PC and download them again. Each device also needs Upload files or Download files. |
| File transfer: File transfer folder | `file_transfer_dir` | empty | Where uploads go and downloads come from. Leave it empty for Downloads\Rubylight of the signed-in user. |
| File transfer: Largest file | `file_transfer_max_mb` | `4096` MiB | Uploads larger than this are refused before they start. |
| File transfer: Transfer speed while streaming | `file_transfer_stream_mbps` | `50` Mbit/s | Uploads and downloads together slow to this rate while a stream runs. 0 lets them run at full speed. |
| Shared folders: Shared folders | `shared_folders` | none | Folders of this PC that devices may browse. Each device sees only the folders you share with it on Devices. |
| Streams: Local network encryption | `lan_encryption_mode` | `0` (Off) | Whether streams to devices on the local network, including Tailscale, are encrypted. Encryption costs some performance on slower devices. |
| Streams: Internet encryption | `wan_encryption_mode` | `1` (When the device supports it) | Whether streams to devices outside the local network are encrypted. Required turns away devices that cannot encrypt. |
| Streams: Device timeout | `ping_timeout` | `10000` ms | How long the host waits without hearing from a device before it ends the stream. Raise it if streams drop on an unreliable network. |
| Streams: Client roaming | `client_roaming` | `true` | Let a device whose network address changes, such as a phone moving from Wi-Fi to mobile data, keep its stream instead of reconnecting. |
| Streams: Bandwidth test | `bandwidth_probe` | `true` | Lets paired devices time a short download to measure the network before they pick a bitrate. |
| Console: Console access | `origin_web_ui_allowed` | `lan` (Local network) | Which addresses may open this console and use its API. |
| Console: Trusted origins | `csrf_allowed_origins` | none | Other web addresses allowed to make changes through this console. Add one only when you reach the console through a reverse proxy. |
| Console: Sign-in token lifetime | `session_token_ttl_seconds` | `7200` s | How long a console sign-in lasts before the browser must renew it. |
| Console: Remembered sign-in lifetime | `remember_me_refresh_token_ttl_seconds` | `604800` s | How long a browser signed in with "Keep me signed in" stays signed in, up to 366 days. Lower it on shared computers. |

## Further reference

- [Features](features.md)
- [Measured performance and testing conditions](../rust/PERFORMANCE.md)
- [Console settings definitions](../rust/web/src/lib/settings-schema.ts), including [video](../rust/web/src/lib/schema/video.ts), [display](../rust/web/src/lib/schema/display.ts) and [general settings](../rust/web/src/lib/schema/basics.ts)
- [Configuration parser and allowed overrides](../rust/core/src/config.rs)
- [Historical C++ configuration reference](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/configuration-cpp.md) — preserved for older installations; its defaults and platform advice do not describe the current Rust host.
