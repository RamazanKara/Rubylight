//! What Nightjar remembers between runs: the client identity, the paired hosts with their
//! pinned certificates, and the settings. JSON files in the platform config directory
//! (`%APPDATA%\Nightjar` on Windows, `~/.config/nightjar` on Linux).

mod art;
mod defaults_gen;
mod enums;
mod host;
mod migrate;
mod override_fields;
mod profiles;
mod store;

#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
use nightjar_policy::http::NativeLibrary;
use serde::{Deserialize, Serialize};

use crate::strings::en;

#[cfg(test)]
use art::art_app;
pub use art::ArtFiles;
pub use defaults_gen::{
    DEADZONE, DESKTOP_OVERRIDE_KEYS, DESKTOP_SETTING_KEYS, DISPLAY_SCALES, GUIDE_HOLD_MS, MIC_GAIN,
    MIC_GAIN_STEP, RENDER_SCALE, SHARP_COLOR_KEYS, UI_ZOOM_STEPS,
};
pub use enums::*;
pub use host::*;
pub use profiles::*;
#[cfg(test)]
use store::HOTKEYS_FILE;
pub use store::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 0x0 is the display's native resolution.
    pub width: u32,
    pub height: u32,
    /// Relative-to-Native resolution scale in percent (100 is full native; 0 reads as 100).
    /// Applies while `width == 0 || height == 0`, preserving the active monitor's aspect ratio.
    pub native_scale: u32,
    /// 0 is the display's refresh rate.
    pub fps: u32,
    /// 0 picks a bitrate for the resolution and rate.
    pub bitrate_kbps: u32,
    /// "Lower bitrate", the one adaptive bitrate setting: The PC decides (default), Lower it sooner
    /// (this device's busy-network controller, or its loss over mid-stream rate changes (0x553C) on
    /// a PC that takes them) or Never: keep my bitrate (a PC that takes rate changes is told once to
    /// hold the stream's own bitrate).
    pub lower_bitrate: nightjar_policy::choices::LowerBitrate,
    /// The three switches Lower bitrate was saved as before 2026-11-20; [`Settings::migrate`] folds
    /// them into `lower_bitrate` once and they are never written again.
    #[serde(rename = "adaptive_bitrate", skip_serializing)]
    #[doc(hidden)]
    pub retired_busy_bitrate: Option<bool>,
    #[serde(rename = "client_hint_bitrate", skip_serializing)]
    #[doc(hidden)]
    pub retired_lost_bitrate: Option<bool>,
    #[serde(rename = "keep_bitrate", skip_serializing)]
    #[doc(hidden)]
    pub retired_keep_bitrate: Option<bool>,
    pub adaptive_quality: QualityMode,
    pub quality_min_short_edge: u32,
    pub quality_min_fps: u32,
    pub codec: CodecPreference,
    pub hdr: bool,
    /// "HDR brightness follows the desktop" (Linux): an HDR stream on a Wayland desktop scales
    /// with the desktop's brightness instead of showing at the brightness the PC sends
    /// (`stream::wl_scanout::Brightness`). Off: the PC's absolute brightness, which HDR10 means
    /// and direct scanout needs.
    pub hdr_desktop_brightness: bool,
    /// "HDR format" (Android's `list_hdr_format`): what an HDR stream may ask for.
    pub hdr_format: nightjar_policy::video_codec::hdr::HdrFormatChoice,
    /// "Sharper colored text" (`nightjar_core::sharp_color`) is one choice kept in two keys that
    /// older versions read: Off is both off, Automatic both on. Every load sets both to "either
    /// was on" ([`Settings::migrate`], [`Overrides`]).
    pub experimental_packed_444: bool,
    /// See `experimental_packed_444`. Older versions read it as "offer the 8-bit 4:4:4 formats, and
    /// with `sdr_10bit` HEVC 4:4:4 10-bit" (`color::offer`).
    pub prefer_yuv444: bool,
    /// "10-bit color (SDR)" (Experimental): offers Main10 without asking for HDR.
    pub sdr_10bit: bool,
    pub audio: AudioChannels,
    pub play_audio_on_host: bool,
    /// "Mute audio": the stream's audio is received and never opened on an output
    /// (`pipeline::audio_renderer`).
    pub mute_audio: bool,
    /// Ask the PC for PCM audio instead of Opus (`pipeline::passthrough_codecs`).
    pub audio_passthrough: bool,
    pub window_mode: WindowMode,
    /// Keep the desktop launcher in an ordinary window even when `window_mode` is Fullscreen or
    /// Borderless; streams still use `window_mode`. Ignored under gamescope and KMS.
    pub windowed_launcher: bool,
    pub video_scaling: VideoScaling,
    pub align_video_top: bool,
    /// The display has variable refresh (G-Sync, FreeSync, HDMI VRR).
    pub vrr: bool,
    /// Real VRR: with VRR, show each frame at the PC's frame time plus a steady delay. On by
    /// default: on screen it put 97% of frames within 1 ms of the game's cadence (81 to 90% shown
    /// when decoded) for 1.3 to 1.5 ms, and over Wi-Fi halved the frames off it for 2.6 to 4.2 ms
    /// (docs/pyrowave-decode.md, "Real VRR").
    pub present_at_host_times: bool,
    /// Tear-free VRR (Windows, with `vrr`): a frame that arrives while the display is still drawing
    /// the last one waits for it to finish, at most one refresh, and is shown whole
    /// (`present::VrrSync::Scan`). On by default; off shows every frame at once, with tearing.
    pub vrr_tear_free: bool,
    /// How Tear-free VRR waits (`VrrWait`, Windows): Automatic by default.
    pub vrr_wait: VrrWait,
    /// Without VRR: wait for the display's refresh (no tearing).
    pub vsync: bool,
    /// Without VRR: how pictures wait for the display (Android's `frame_pacing`).
    pub frame_pacing: FramePacing,
    /// Balanced's "keep only the newest frame" (Android's `checkbox_drop_late_frames`), one of the
    /// Frame pacing row's choices: the newest picture at each refresh. Off ("Balanced, keep every
    /// frame"): each picture in turn, one a refresh, as Smoothest video waits
    /// (`stream::present::Pacer::take_due`). On by default.
    pub drop_late_frames: bool,
    /// "PC's frame times" without VRR (Android's `checkbox_host_frame_times`), a choice of the Frame
    /// pacing row that takes over from the pacing: each picture waits for its time on the PC's
    /// clock plus a delay learnt from recent frames, and goes out for the refresh that time falls
    /// on, with V-Sync (`stream::present::Pacer::accept`). Off by default; a file without it reads
    /// as off.
    pub host_frame_times: bool,
    /// Fractional V-Sync (Android's `checkbox_fractional_vsync`): with V-Sync pacing on a display
    /// that refreshes at 2 to 4 times the stream's rate, each picture stays for exactly that many
    /// refreshes (`nightjar_policy::vsync::refreshes_per_frame`). The client holds each picture until
    /// the refresh that opens its turn and keeps the swap chain's sync interval at 1.
    pub fractional_vsync: bool,
    /// The performance overlay is on (Android's `checkbox_enable_perf_overlay`), showing
    /// `overlay_mode` in the `overlay_position` corner. The stream menu and its shortcut change
    /// these and keep them.
    pub overlay: bool,
    pub overlay_mode: OverlayMode,
    pub overlay_position: OverlayCorner,
    /// Append this device's battery percentage and local clock time to the compact performance overlay.
    pub compact_overlay_device_info: bool,
    pub mouse_mode: MouseMode,
    pub mouse_speed: u32,
    pub mouse_nav_buttons: bool,
    /// A touchpad's fingers go to a PC that takes touchpad contacts (Foundation Sunshine,
    /// Rubylight) instead of the pointer the system makes of them: Raw Input on Windows, evdev
    /// on Linux, the trackpad's touches on macOS.
    pub native_touchpad: bool,
    pub right_alt_as_meta: bool,
    /// Send Alt+Tab, the Windows key and similar to the host while the stream has focus.
    pub capture_system_keys: bool,
    /// With `capture_system_keys`: only while the stream's window is full screen (the Fullscreen
    /// and Borderless window modes); in a window those keys stay with this computer.
    pub system_keys_fullscreen_only: bool,
    /// "Controllers" not Off ([`nightjar_policy::controllers::Controllers`]): controller input goes to the PC.
    pub controllers: bool,
    /// "Controllers" A player each: every controller is its own player; off sends them all as
    /// player 1. Kept while `controllers` is off. On by default, as before it was a choice.
    pub multi_controller: bool,
    /// Nintendo layout: swap A/B and X/Y.
    pub swap_face_buttons: bool,
    pub controller_motion: bool,
    pub controller_audio: bool,
    pub back_opens_menu: bool,
    /// "Back button as Guide": the controller's Back (Select, View) sends Guide to the PC (Android's
    /// `checkbox_back_as_guide`). Off by default, as on Android.
    pub back_as_guide: bool,
    /// Holding the controller's Guide (Home) button this long opens the stream menu; a shorter
    /// press still goes to the PC. 0 is off (one of [`GUIDE_HOLD_MS`], Android's stored values).
    pub guide_hold_ms: u32,
    pub gamepad_quit_combo: GamepadQuitCombo,
    /// What Y, X, LB and RB run while the stream menu is open; a button not here keeps its own.
    pub stream_menu_buttons: nightjar_policy::stream_menu::Buttons,
    pub controller_mouse: bool,
    pub analog_scrolling: AnalogScrolling,
    pub mouse_fixed_step: bool,
    pub gamepad_touchpad_as_mouse: bool,
    /// "Stick dead zone" in percent of the stick's travel, for every controller whose buttons
    /// page sets none of its own (`gamepad::Mapping::deadzone` 0). 0 by default, as before.
    pub deadzone: u32,
    /// "Trigger dead zone" in percent, likewise for `gamepad::Mapping::trigger_deadzone`.
    pub trigger_deadzone: u32,
    pub disable_rumble: bool,
    /// "Rumble feel": the curve the host's motor levels pass through (`nightjar_policy::rumble`, Android's `list_rumble_feel`).
    pub rumble_feel: nightjar_policy::rumble::RumbleFeel,
    /// Keep sending controller input while the window is in the background.
    pub background_controllers: bool,
    pub decoder: DecoderPreference,
    /// PyroWave: decode the records of a frame while the rest of it is still on the wire,
    /// instead of when the frame is complete. Off by default until measured on the wired
    /// client (docs/early-decode.md). `NIGHTJAR_EARLY_DECODE=1` or `0` overrides it.
    pub early_decode: bool,
    /// EXPERIMENTAL. PyroWave on a wired link: ask the host to send the picture's records in row
    /// order and present the picture in slices ahead of the display's beam (docs/beam-racing.md).
    /// Implies early decode. Off by default, and off costs nothing: no row order is requested, no
    /// scan line probe runs, frames decode whole. It missed its default-on gate on loopback (it
    /// helps the median picture age at a low pace, not the p95, and `receive_to_present` is worse
    /// at a high pace; docs/pyrowave-decode.md). When it cannot run it says why in the log and the
    /// stats overlay. `NIGHTJAR_BEAM_RACE=1` or `0` overrides it.
    pub beam_race: bool,
    /// How far ahead of the display's beam a slice must be presented when the beam race is on, in
    /// ms; 0 works it out from how the presents vary (docs/beam-racing.md). `NIGHTJAR_BEAM_RACE_MARGIN_MS`
    /// overrides it.
    pub beam_race_margin_ms: u32,
    /// PyroWave: reconstruct on the GPU with the PyroWave library (the default, where it measured
    /// better on every leg, docs/pyrowave-decode.md) or on the CPU.
    pub pyrowave_decoder: PyroWaveDecoder,
    /// What draws the window on Linux ([`Renderer`]); for the whole app, not a PC's or a profile's.
    pub renderer: Renderer,
    /// The GPU on a Windows PC with two ([`GraphicsCard`]); for the whole app.
    pub graphics_card: GraphicsCard,
    /// Where Nightjar shows itself on Linux ([`DisplayOutput`]); for the whole app, read at start.
    pub display_output: DisplayOutput,
    /// Drawing straight to the display, a stream's decoded pictures go on a display plane
    /// (`stream::kms_scanout`) instead of being drawn into the window; for the whole app.
    pub video_planes: bool,
    /// "High-performance GPU power while streaming" (Linux): each AMD GPU that drives a connected
    /// display is held at its peak power state for the stream (`stream::gpu_hold`). Off by default;
    /// for the whole app.
    pub high_performance_gpu_power: bool,
    pub microphone: bool,
    /// Echo cancellation on the microphone (`stream::microphone`), automatic by default.
    pub mic_echo_cancel: MicEcho,
    /// "Microphone gain" in percent, applied before the microphone is encoded (Android's
    /// `seekbar_mic_gain`, 25-400 in steps of 5). 100 by default: unchanged.
    pub mic_gain: u32,
    /// "Noise suppression" on the microphone (Android's `checkbox_mic_noise_suppression`):
    /// WebRTC's noise suppressor before Opus (`stream::mic_dsp`), or the Windows voice DSP's own
    /// with echo cancellation. On by default, as on Android and as that DSP already runs it.
    pub mic_noise_suppression: bool,
    /// "Automatic gain" on the microphone (Android's `checkbox_mic_auto_gain`): WebRTC's AGC2
    /// adaptive digital gain before Opus (`stream::mic_dsp`). Off by default.
    pub mic_auto_gain: bool,
    pub cursor_shapes: bool,
    /// "Wait through network dropouts": one of `nightjar_policy::recovery::NETWORK_GRACE_CHOICES_S`
    /// in ms, or Off (anything up to the connection's own timeout; 3 s by default).
    pub network_grace_ms: u32,
    /// "Hide connection warnings": a stream shows no warnings about a poor connection (Android's
    /// `checkbox_disable_warnings`). Off by default.
    pub hide_connection_warnings: bool,
    /// Client roaming (Experimental): keep the stream when this device changes networks, with a
    /// Rubylight PC that supports it. On by default.
    pub client_roaming: bool,
    /// Let client roaming move the stream onto a metered network. Off by default.
    pub roaming_metered: bool,
    /// "Group incoming video packets" (Android's `checkbox_receive_coalescing`): the network
    /// stack may hand over several video packets in one read, receive coalescing on Windows and
    /// UDP_GRO on Linux (`SessionConfig::receive_coalescing`). On by default.
    pub receive_coalescing: bool,
    /// Jumbo video packets (Android's `jumbo_frames`): on a PC that offers more than 1400-byte
    /// packets and a stream whose pre-flight probe passes at the larger size, video packets carry
    /// up to the offer (8192 at most). Fewer, larger packets save CPU and interrupts at high
    /// bitrates; every hop must carry 9000-byte frames. Off by default.
    pub jumbo_frames: bool,
    /// `--packet-size`: a video packet size for this run only, including the encryption header
    /// (0 follows the stream's route and the setting). Never above the PC's per-stream offer.
    #[serde(skip)]
    pub packet_size: u32,
    /// "Play over the internet" (Android's `checkbox_internet_play`): after the local network,
    /// IPv6 and a forwarded port, reach a PC that offers it through a punched path
    /// (`nightjar_core::internet`). On by default: at home the local network answers first.
    pub internet_play: bool,
    /// USB forwarding (Experimental): attach USB devices plugged in here to the PC from the
    /// stream controls (src/usb.rs). Off by default.
    pub usb_forwarding: bool,
    /// The USB devices (vendor:product, `usb::auto_key`) attached to the PC whenever they are
    /// plugged in during a stream; ticked per device in the stream controls. Empty by default.
    pub usb_auto: Vec<String>,
    /// Clipboard sync: what is copied here goes on the PC's clipboard and the PC's copies come
    /// here, as text, with a Rubylight PC that allows this device its clipboard (0x5508,
    /// stream/clipboard_sync.rs). Off by default.
    pub clipboard_sync: bool,
    /// "Keyboard follows text fields" (Android's `checkbox_keyboard_follows_text_fields`): a PC that
    /// reports its text fields (0x553B) is asked to, so the on-screen keyboard opens and closes
    /// with them where SDL has one (stream/text_field.rs). On by default.
    pub keyboard_follows_text_fields: bool,
    /// Dual screen mode (Android's `checkbox_dual_screen`): with a second monitor and a PC that keeps
    /// a device's second stream beside its first, a stream starts a second one of the same game,
    /// full screen on the other monitor (`second_screen.rs`). Off by default.
    pub dual_screen: bool,
    /// The second screen's resolution (Android's `list_resolution_screen2`); 0x0 is its monitor's
    /// own size.
    pub second_screen_width: u32,
    pub second_screen_height: u32,
    /// Tell a Rubylight PC when the route to it is Wi-Fi or cellular, so it paces for a wireless
    /// link from the first frame (`x-rl-video[0].clientWireless`). On by default.
    pub tell_host_wireless: bool,
    /// Tell the PC this device's wired port speed toward it, so it can limit its sending rate
    /// (`x-ss-video[0].pyrowaveLinkMbps`, any codec, any host). On by default. The calibrated pace
    /// is looked up with the real link speed either way.
    pub tell_host_link_speed: bool,
    /// Send mouse and pen motion at once instead of batching it for 1 ms (Android's "Instant
    /// mouse and pen input"). On by default, as on Android.
    pub unbatched_input: bool,
    /// "Prioritize stream traffic": socket and thread priority for the stream, on Linux and
    /// Windows (`priority.rs`). On by default, as on Android.
    pub network_priority: bool,
    /// Debug: software decoding asks the PC for slices its threads decode in parallel. Off by
    /// default: it measured worse (`NIGHTJAR_DESKTOP_SLICES` overrides it for test runs).
    pub software_slices: bool,
    /// Debug, Linux: the GPU renderer tries Vulkan video before VAAPI. Off by default: VAAPI's
    /// mapping into Vulkan is zero-copy too and keeps reference frame invalidation
    /// (`NIGHTJAR_VULKAN_VIDEO_GPU_FIRST` overrides it).
    pub vulkan_video_first: bool,
    /// Debug, Linux: Vulkan video recovers lost frames with reference frame invalidation. Off by
    /// default, unmeasured (`NIGHTJAR_VULKAN_RFI` overrides it).
    pub vulkan_rfi: bool,
    /// Debug, macOS: VideoToolbox recovers lost frames with reference frame invalidation. Off by
    /// default, unmeasured on a Mac (`NIGHTJAR_VIDEOTOOLBOX_RFI` overrides it).
    pub videotoolbox_rfi: bool,
    /// Video: PyroWave's GPU decoder decodes on a separate compute queue where the GPU has one
    /// (`stream::pyrowave_sdk::queue`); without one it changes nothing. Off by default: the client
    /// it is ported from reverted it for delaying Wayland presentation, and the ABBA on the 780M
    /// laptop showed no clear gain (docs/pyrowave-decode.md). On Linux the GPU renderer's
    /// device gets the queue when it is made, so the setting takes effect there from the next start.
    pub pyrowave_async_compute: bool,
    /// "Recover interrupted streams". On by default, as on Android.
    pub auto_reconnect: bool,
    /// The once-only switch of `auto_reconnect` to on has run (Android's
    /// `auto_reconnect_default_on`). Missing from files written before it, so those are switched
    /// once; a choice made after that is kept.
    #[serde(default)]
    pub auto_reconnect_default_on: bool,
    /// The once-only switch of Real VRR (`present_at_host_times`) to on has run (Android's
    /// `checkbox_real_vrr` is a new key instead). Missing from files written before it, so those
    /// are switched once; a choice made after that is kept.
    #[serde(default)]
    pub real_vrr_default_on: bool,
    /// The once-only look at the PCs' and games' own settings has run: before 4.1.1, pressing A
    /// on an inherited row pinned its value for the whole PC, and a pin can't be told apart from
    /// a choice (`Launcher::show_kept_settings_once`). Missing from files written before it.
    #[serde(default)]
    pub kept_settings_checked: bool,
    pub virtual_display: bool,
    /// "PC render resolution": the percent of the stream resolution the PC renders its virtual
    /// display at (`scaleFactor`), as on Android. Kept in `RENDER_SCALE` when read: older versions
    /// offered up to 300 %.
    #[serde(deserialize_with = "migrate::render_scale")]
    pub virtual_display_scale: u32,
    /// "PC display scaling": the Windows scale (text and interface size) of the virtual
    /// display, in percent (`rlDisplayScale`); 0 keeps the PC's own setting. A saved scale
    /// Windows does not offer reads as 0, as on Android: the PC would refuse the launch.
    #[serde(deserialize_with = "migrate::display_scale")]
    pub virtual_display_dpi: u32,
    pub host_display: HostDisplay,
    /// The video FEC asked for at launch from a PC that lets the client choose it.
    pub error_correction: ErrorCorrection,
    /// RTX HDR for this PC's streams (`rlRtxHdr`): on or off over the PC's app setting, or
    /// `None` to follow it. Offered on a PC's own settings when its host reports it
    /// (`NvidiaEnhancements`).
    pub rtx_hdr: Option<bool>,
    /// RTX Video enhancement for this PC's streams (`rlRtxVideo`), as `rtx_hdr`.
    pub rtx_video: Option<bool>,
    /// Link test before the first bitrate: a new stream at the automatic bitrate
    /// times `GET /bandwidth-probe` first and starts at no more than 80 % of it. Off by default:
    /// on the NUC it started streams lower with more loss and freezes (rust/PERFORMANCE.md).
    pub link_probe: bool,
    /// "Game optimization hint": the launch lets the PC adjust the game's settings for the stream
    /// (`sops`). On by default, as the launch always asked before it was a setting.
    pub sops: bool,
    /// "Frames waiting for the decoder" (Android's `list_decode_queue_frames`): complete frames
    /// that may wait for the decoder before they are dropped and a key frame is requested
    /// (`SessionConfig::decode_queue_bound`). 15 by default, the engine's bound before it was a
    /// setting; the engine reads 0 or more than 15 as 15.
    pub decode_queue_frames: u32,
    pub full_range: bool,
    pub upscaling: Upscaling,
    pub upscale_sharpness: u32,
    /// Frame Warp (Android's `checkbox_frame_warp`): with a PC that offers it, each present moves
    /// the picture by the mouse and right-stick input the PC had not seen when it rendered it,
    /// keeping the HUD in place (`stream::frame_warp`). Off by default.
    pub frame_warp: bool,
    /// What a cold start opens (Android's `list_open_on_start`). Not a per-PC or per-game override.
    pub open_on_start: OpenOnStart,
    /// Open on Home (Continue playing, the library across PCs) when a paired PC has the
    /// Rubylight native API. Off starts on the list of PCs. Not a per-PC or per-game override.
    pub console_home: bool,
    /// The simple layer's choices (Android's `simple_smoothness`, `simple_picture`, `simple_hdr`):
    /// three plain settings that write the concrete values above when a player picks one in Settings.
    /// A player who never opens the simple layer keeps every default above untouched.
    pub simple_smoothness: nightjar_policy::simple_profile::Smoothness,
    pub simple_picture: nightjar_policy::simple_profile::Picture,
    pub simple_hdr: nightjar_policy::simple_profile::Hdr,
    /// A click on a PC that runs a game goes straight back into it instead of opening its
    /// library (Android's `checkbox_quick_resume`). Not a per-PC or per-game override.
    pub quick_resume: bool,
    /// Offer Input-only in a game's menu on PCs whose host has no Input-only mode of its own: a
    /// small ordinary stream whose video and audio are not shown or played (Android's
    /// `checkbox_input_only_any_host`). Not a per-PC or per-game override.
    pub input_only_any_host: bool,
    /// Home's View: its library rows as lines or whole, by store or as one, and the cover size.
    pub home_layout: HomeLayout,
    pub home_group: HomeGroup,
    pub home_card_size: CardSize,
    /// What Home's rows and All games sort by, and which way.
    pub home_sort: HomeSort,
    pub home_order: HomeOrder,
    /// Settings' "More settings" fold, as the user last chose it (Android keeps it the same way): no
    /// setting, so no row, reset, profile or per-PC or per-game value has it. A search or a link
    /// that opens the fold for a section behind it does not change it.
    pub settings_more_open: bool,
    /// The PC Home's "Set up for this device and …" card is about, offered once after the first
    /// pairing and cleared when dismissed. No setting, as `settings_more_open`.
    pub setup_card_host: Option<u64>,
    /// The card was offered on this install: it never comes back, even after every PC is unpaired.
    pub setup_card_seen: bool,
    /// The launcher's size in percent: one of [`UI_ZOOM_STEPS`], or 0 for Auto, which follows the
    /// screen's height ([`auto_zoom`]). Not a per-PC or per-game override.
    pub ui_zoom: u32,
    /// The once-only move of the old default 100 % to Auto has run ([`Settings::migrate`]).
    /// Present from new installs; missing from files written before Auto, so those move their old
    /// default 100 % once, and a 100 % chosen after that is kept.
    #[serde(default)]
    pub ui_zoom_auto_migrated: bool,
    /// Brighter text, black surfaces and thicker outlines and focus rings.
    pub high_contrast: bool,
    /// "Show debug settings" (Android's `checkbox_show_debug_settings`): the settings list shows the
    /// Debug section. It shows anyway while a Debug setting is not at its default. Not a per-PC or
    /// per-game override.
    pub show_debug_settings: bool,
    /// "Theme" (Android's `ThemeChoice`, stored there as `theme`; the switches
    /// `checkbox_always_dark` and `checkbox_oled_black` before): the system's light or dark, the
    /// night look, or OLED black.
    pub theme: Theme,
    pub reduce_motion: ReduceMotion,
    pub language: Language,
    /// Windows and AppImage: look for a newer stable release once per launch (update_check.rs).
    pub check_updates: bool,
    /// "Show decode time after streaming" (Android's `checkbox_enable_post_stream_toast`): when a
    /// stream ends, a notice gives its average decode time and codec. Off by default.
    pub post_stream_decode_time: bool,
}

impl Default for Settings {
    fn default() -> Self {
        defaults_gen::default_settings()
    }
}

/// The step to use for a stored size: the nearest one, so a hand-edited file still works. 0 is
/// Auto and stays 0.
pub fn nearest_zoom(percent: u32) -> u32 {
    if percent == 0 {
        return 0;
    }
    UI_ZOOM_STEPS
        .into_iter()
        .min_by_key(|step| step.abs_diff(percent))
        .unwrap_or(100)
}

/// Auto's size for a monitor `height` in egui points and the OS's display scale. Where the OS
/// scales (above 1), it already sized everything for the screen and Auto stays at 100 %, so a
/// scaled desktop looks as it did. Unscaled, a 4K-class screen (2000 points or taller, such as a
/// TV under gamescope) gets its height over 1080, clamped to 250 %; smaller screens and a screen
/// of unknown size stay at 100 %.
pub fn auto_zoom(height_points: Option<f32>, os_scale: f32) -> u32 {
    let Some(height) = height_points.filter(|h| h.is_finite()) else {
        return 100;
    };
    if os_scale > 1.01 || height < 2000.0 {
        return 100;
    }
    ((height / 1080.0).clamp(1.0, 2.5) * 100.0).round() as u32
}

/// The step after (`+1`) or before (`-1`) `percent`, kept inside the steps.
pub fn step_zoom(percent: u32, direction: i32) -> u32 {
    let at = UI_ZOOM_STEPS
        .iter()
        .position(|s| *s == nearest_zoom(percent))
        .unwrap_or(1);
    let at = (at as i32 + direction).clamp(0, UI_ZOOM_STEPS.len() as i32 - 1);
    UI_ZOOM_STEPS[at as usize]
}

impl Settings {
    /// Whether these settings draw through the GPU renderer, which Linux picks at start: SDL's
    /// "gpu" renderer runs the shader passes (packed full chroma, HDR, PyroWave, upscaling, and
    /// the plane pass that 4:4:4 and 10-bit pictures need).
    pub fn needs_gpu(&self) -> bool {
        self.gpu_need().is_some()
    }

    /// What in these settings needs the GPU renderer, for the log of a renderer switch; HDR comes
    /// last, so "HDR" means nothing else does.
    pub fn gpu_need(&self) -> Option<&'static str> {
        [
            (self.experimental_packed_444, "packed full chroma"),
            (self.codec.is_pyrowave(), en("PyroWave")),
            (self.upscaling.is_shader() || self.upscaling == Upscaling::Ai, "upscaling"),
            (self.prefer_yuv444, "4:4:4"),
            (self.sdr_10bit, "10-bit SDR"),
            (self.hdr, en("HDR")),
        ]
        .into_iter()
        .find_map(|(on, why)| on.then_some(why))
    }
    /// Whether setting key `key` differs between `self` and `other`, without JSON serialization.
    pub fn field_differs(&self, other: &Self, key: &str) -> bool {
        override_fields::field_differs(self, other, key).unwrap_or(false)
    }
}

/// The settings page's name for what [`Settings::gpu_need`] found, for a refusal to translate.
pub fn gpu_need_label(need: &'static str) -> &'static str {
    match need {
        "packed full chroma" => en("Sharper colored text"),
        "upscaling" => en("Upscaling"),
        "4:4:4" => en("Sharper colored text"),
        "10-bit SDR" => en("Less color banding"),
        other => other,
    }
}

/// A bitrate for a stream of this size and rate, scaled from 20 Mbps at 1080p60 as
/// moonlight-qt's default does: linear in pixels, with rates above 60 counting for less.
pub fn default_bitrate_kbps(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = f64::from(width.max(1)) * f64::from(height.max(1));
    let fps = f64::from(fps.max(1));
    let rate = if fps <= 60.0 {
        fps / 60.0
    } else {
        (fps / 60.0).sqrt()
    };
    let kbps = 20_000.0 * pixels / (1920.0 * 1080.0) * rate;
    (kbps.round() as u32).clamp(2_000, 150_000)
}

/// Translatable UI labels used by `config::enums` and `config::profiles`, kept in `config.rs`
/// so `strings::tests` scans every `en(...)` literal in one place.
pub(super) mod labels {
    use crate::strings::en;

    pub const FIT: &str = en("Fit");
    pub const FILL_CROP: &str = en("Fill (crop)");
    pub const STRETCH: &str = en("Stretch");
    pub const NATIVE_PIXELS: &str = en("1:1 pixels");
    pub const CPU: &str = en("CPU");
    pub const GPU: &str = en("GPU");
    pub const AUTOMATIC: &str = en("Automatic");
    pub const DESKTOP: &str = en("Desktop");
    pub const STRAIGHT_TO_DISPLAY: &str = en("Straight to the display");
    pub const POWER_SAVING: &str = en("Power saving");
    pub const HIGH_PERFORMANCE: &str = en("High performance");
    pub const STANDARD: &str = en("Standard");
    pub const STRONG: &str = en("Strong");
    pub const SUBTLE: &str = en("Subtle");
    pub const NIGHTJARS_WAIT: &str = en("Nightjar’s wait");
    pub const WINDOWS_VSYNC: &str = en("Windows V-Sync");
    pub const LOWEST_LATENCY: &str = en("Lowest latency");
    pub const BALANCED: &str = en("Balanced");
    pub const BALANCED_FPS_LIMIT: &str = en("Balanced with FPS limit");
    pub const SMOOTHEST_VIDEO: &str = en("Smoothest video (more delay)");
    pub const ON: &str = en("On");
    pub const OFF: &str = en("Off");
    pub const SYSTEM: &str = en("System");
    pub const DARK: &str = en("Dark");
    pub const OLED_BLACK: &str = en("OLED black");
    pub const LIGHT: &str = en("Light");
    pub const PREFER_MOTION: &str = en("Prefer motion");
    pub const PREFER_DETAIL: &str = en("Prefer detail");
    pub const SAVE_POWER: &str = en("Save power");
    pub const AI_ENHANCEMENT: &str = en("AI enhancement");
    pub const COMPACT: &str = en("Compact");
    pub const STATISTICS: &str = en("Statistics");
    pub const DEBUG: &str = en("Debug");
    pub const TOP_LEFT: &str = en("Top left");
    pub const TOP_RIGHT: &str = en("Top right");
    pub const BOTTOM_LEFT: &str = en("Bottom left");
    pub const BOTTOM_RIGHT: &str = en("Bottom right");
    pub const SHOW_MY_PCS: &str = en("Show my PCs");
    pub const CONTINUE_LAST_GAME: &str = en("Continue my last game");
    pub const CHOSEN_GAME: &str = en("A chosen game");
    pub const GRID: &str = en("Grid");
    pub const LIST: &str = en("List");
    pub const ROWS: &str = en("Rows");
    pub const STORE: &str = en("Store");
    pub const TYPE: &str = en("Type");
    pub const COLLECTION: &str = en("Collection");
    pub const INSTALLATION: &str = en("Installation");
    pub const NONE: &str = en("None");
    pub const TITLE: &str = en("Title");
    pub const LAST_PLAYED: &str = en("Last played");
    pub const PLAY_TIME: &str = en("Play time");
    pub const ASCENDING: &str = en("Ascending");
    pub const DESCENDING: &str = en("Descending");
    pub const SMALL: &str = en("Small");
    pub const MEDIUM: &str = en("Medium");
    pub const LARGE: &str = en("Large");
    pub const PROFILE_NAME_LEN: &str = en("Use a name between 1 and 32 characters.");
    pub const PROFILE_NAME_TAKEN: &str = en("A profile already uses that name.");
    pub const PROFILE_LIMIT: &str = en("You can save up to 50 profiles.");
    pub const PROFILE_NO_IDS: &str = en("No profile IDs available.");
}

#[cfg(test)]
mod tests;
