//! What Nightjar remembers between runs: the client identity, the paired hosts with their
//! pinned certificates, and the settings. JSON files in the platform config directory
//! (`%APPDATA%\Nightjar` on Windows, `~/.config/nightjar` on Linux).

use std::path::{Path, PathBuf};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use nightjar_core::http::NativeLibrary;
use serde::{Deserialize, Serialize};

use crate::strings::en;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodecPreference {
    Auto,
    H264,
    Hevc,
    Av1,
    PyroWave,
    PyroWave444,
}

impl CodecPreference {
    pub fn is_pyrowave(self) -> bool { matches!(self, Self::PyroWave | Self::PyroWave444) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioChannels {
    Stereo,
    Surround51,
    Surround71,
    /// Offered where the output plays it (`stream::offers_714`); asked for only from a PC that
    /// reports it.
    Surround714,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowMode {
    /// Exclusive fullscreen at the stream's mode when the display has it.
    Fullscreen,
    /// A borderless window covering the display.
    Borderless,
    Windowed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseMode {
    /// The pointer is captured and moves relatively, for games.
    Capture,
    /// The local pointer is the host's pointer, for the desktop.
    Absolute,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoScaling {
    #[default]
    Fit,
    Fill,
    Stretch,
    Native,
}

impl VideoScaling {
    pub fn label(self) -> &'static str {
        match self { Self::Fit => en("Fit"), Self::Fill => en("Fill (crop)"), Self::Stretch => en("Stretch"), Self::Native => en("1:1 pixels") }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalogScrolling {
    Off,
    #[default]
    Right,
    Left,
}

/// The controller chord that leaves the stream (V+ PC's gamepad quit combo, ported as is). The
/// alternatives are for controllers or games where Start+Select+LB+RB is taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GamepadQuitCombo {
    #[default]
    StartSelectLbRb,
    SelectLbRbY,
    StartLbRbA,
    StartLbRbB,
    LbRbXY,
    LbRbAB,
}

/// The choices for holding Guide to open the stream menu, in milliseconds; 0 is off.
pub const GUIDE_HOLD_MS: [u32; 5] = [0, 500, 1000, 1500, 2000];

impl GamepadQuitCombo {
    /// Their names follow the connected controller (`glyphs::Style::quit_combo`).
    pub const ALL: [Self; 6] = [Self::StartSelectLbRb, Self::SelectLbRbY, Self::StartLbRbA, Self::StartLbRbB, Self::LbRbXY, Self::LbRbAB];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecoderPreference {
    Auto,
    Software,
}

/// Where PyroWave frames are reconstructed: on the CPU with the Rust decoder, or on the GPU
/// by the PyroWave library's Vulkan decoder (docs/pyrowave-decode.md). `Gpu` never falls back
/// to the CPU: when the library or the adapter cannot do it, PyroWave is not offered and the
/// reason is shown. The default is `Gpu` on Windows and Linux (the ABBAs in docs/pyrowave-decode.md:
/// lower `receive_to_present_ms` at p50 and p95 and no lower fps on every leg). A Linux client
/// without `libpyrowave-shared.so` (a source build; `package-linux.sh -s` ships it) fails the probe
/// like a Windows one without the DLL: PyroWave is not offered, with the reason and the way out
/// (switch to `Cpu`); every other codec works.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PyroWaveDecoder {
    #[cfg_attr(target_os = "macos", default)]
    Cpu,
    #[cfg_attr(not(target_os = "macos"), default)]
    Gpu,
}

impl PyroWaveDecoder {
    pub fn label(self) -> &'static str {
        match self { Self::Cpu => en("CPU"), Self::Gpu => en("GPU") }
    }
}

/// What draws the window on Linux (Windows always draws with Direct3D 11). `Auto` starts on OpenGL,
/// where VAAPI pictures arrive zero-copy (`dmabuf`), unless a saved setting needs SDL's "gpu"
/// renderer, and moves to it when a stream needs it (`handheld::renderer_switch`); `OpenGl` never
/// leaves OpenGL, so such a stream is refused; `Gpu` draws with the GPU renderer from the start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Renderer {
    #[default]
    Auto,
    OpenGl,
    Gpu,
}

impl Renderer {
    pub const ALL: [Self; 3] = [Self::Auto, Self::OpenGl, Self::Gpu];

    /// The menu's name; the two renderers are named as their APIs, in every language.
    pub fn label(self) -> &'static str {
        match self { Self::Auto => en("Automatic"), Self::OpenGl => "OpenGL", Self::Gpu => "GPU (Vulkan)" }
    }
}

/// Which GPU draws and decodes on a Windows PC with two (`gpu_adapters`), for the whole app:
/// Automatic is Windows' choice, unless YUV 4:4:4 is asked for and only the high-performance GPU
/// decodes it; the others are Windows' per-app preference, set for Nightjar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GraphicsCard {
    #[default]
    Automatic,
    PowerSaving,
    HighPerformance,
}

impl GraphicsCard {
    #[cfg_attr(not(windows), allow(dead_code))] // the menu is drawn only on Windows
    pub const ALL: [Self; 3] = [Self::Automatic, Self::PowerSaving, Self::HighPerformance];

    pub fn label(self) -> &'static str {
        match self { Self::Automatic => en("Automatic"), Self::PowerSaving => en("Power saving"), Self::HighPerformance => en("High performance") }
    }
}

/// Frame pacing without VRR, Android's `frame_pacing` (`FramePacingPolicy`): how decoded pictures
/// wait for the display. `stream::present` applies it; with VRR it does not apply, as on Android.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FramePacing {
    /// The pacing before this setting: the newest picture at each refresh, or each picture at once
    /// with V-Sync off. Adds no delay.
    #[default]
    Latency,
    /// V-Sync whatever the V-Sync setting says: the newest picture at each refresh, no tearing.
    Balanced,
    /// V-Sync, and no more pictures a second than the stream's frame rate: a picture that comes
    /// early waits for its slot (at most one frame) instead of replacing the one before it.
    CapFps,
    /// V-Sync, every picture in turn, one a refresh: a picture is skipped only once it is more
    /// than a frame old and a newer one is due.
    Smoothness,
}

impl FramePacing {
    pub const ALL: [Self; 4] = [Self::Latency, Self::Balanced, Self::CapFps, Self::Smoothness];

    pub fn label(self) -> &'static str {
        match self {
            Self::Latency => en("Lowest latency"),
            Self::Balanced => en("Balanced"),
            Self::CapFps => en("Balanced with FPS limit"),
            Self::Smoothness => en("Smoothest video (more delay)"),
        }
    }

    /// Every mode but `Latency` waits for the refresh.
    pub fn forces_vsync(self) -> bool {
        self != Self::Latency
    }
}

/// Microphone echo cancellation, Android's `list_mic_echo_cancel` (`MicProcessing.Echo`):
/// automatic leaves it off only when this device plays through headphones or a headset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicEcho {
    #[default]
    Auto,
    On,
    Off,
}

impl MicEcho {
    pub const ALL: [Self; 3] = [Self::Auto, Self::On, Self::Off];

    pub fn label(self) -> &'static str {
        match self { Self::Auto => en("Automatic"), Self::On => en("On"), Self::Off => en("Off") }
    }

    /// Whether echo cancellation runs: `private_output` is a playback device that keeps the
    /// stream's sound out of the microphone (headphones, a headset), when that is known.
    pub fn wanted(self, private_output: bool) -> bool {
        self == Self::On || (self == Self::Auto && !private_output)
    }
}

/// "Theme" (Android's `ThemeChoice`): System follows the system's light or dark setting, Dark is
/// the night look, OLED black its black-surfaced variant. High contrast wins over all three.
/// Dark by default: the look desktop Nightjar had before the choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    System,
    #[default]
    Dark,
    Oled,
}

impl Theme {
    pub const ALL: [Self; 3] = [Self::System, Self::Dark, Self::Oled];

    /// Android's `theme_choice_names`.
    pub fn label(self) -> &'static str {
        match self { Self::System => en("System"), Self::Dark => en("Dark"), Self::Oled => en("OLED black") }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualityMode {
    #[default]
    Off,
    Motion,
    Detail,
    Battery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Upscaling {
    #[default]
    Bilinear,
    Fsr1,
    Sgsr1,
    /// The GPU vendor's video super resolution and, on NVIDIA, RTX Video HDR (`stream::enhance`);
    /// offered only where the probe finds them (Direct3D 11).
    Ai,
}

impl Upscaling {
    pub fn label(self) -> &'static str {
        match self { Self::Bilinear => "Bilinear", Self::Fsr1 => "FSR 1", Self::Sgsr1 => "SGSR 1", Self::Ai => en("AI enhancement") }
    }

    /// The shader upscalers (`stream::upscale`), which take the sharpness setting.
    pub fn is_shader(self) -> bool {
        matches!(self, Self::Fsr1 | Self::Sgsr1)
    }
}

impl QualityMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => en("Off"),
            Self::Motion => en("Prefer motion"),
            Self::Detail => en("Prefer detail"),
            Self::Battery => en("Save power"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 0x0 is the display's native resolution.
    pub width: u32,
    pub height: u32,
    /// 0 is the display's refresh rate.
    pub fps: u32,
    /// 0 picks a bitrate for the resolution and rate.
    pub bitrate_kbps: u32,
    pub adaptive_bitrate: bool,
    /// Steers the bitrate from this client's loss over mid-stream rate changes (0x553C), on a
    /// PC that takes them, in place of `adaptive_bitrate`. Off by default.
    pub client_hint_bitrate: bool,
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
    pub video_scaling: VideoScaling,
    pub align_video_top: bool,
    /// The display has variable refresh (G-Sync, FreeSync, HDMI VRR).
    pub vrr: bool,
    /// With VRR: show each frame at the PC's frame time plus a steady delay. Off by default: on a
    /// 240 Hz tearing display it held PyroWave frames about 7 ms (receive_to_present p50 10.4 ms
    /// against 3.2 ms shown immediately) and its present intervals were less even
    /// (docs/pyrowave-decode.md, "The wait between decoded and shown").
    pub present_at_host_times: bool,
    /// Without VRR: wait for the display's refresh (no tearing).
    pub vsync: bool,
    /// Without VRR: how pictures wait for the display (Android's `frame_pacing`).
    pub frame_pacing: FramePacing,
    /// "Keep only the newest frame" (Android's `checkbox_drop_late_frames`): with Balanced frame
    /// pacing, the newest picture at each refresh. Off: each picture in turn, one a refresh, as
    /// Smoothest video waits (`stream::present::Pacer::take_due`). On by default.
    pub drop_late_frames: bool,
    /// The performance overlay is on (Android's `checkbox_enable_perf_overlay`), showing
    /// `overlay_mode` in the `overlay_position` corner. The stream menu and its shortcut change
    /// these and keep them.
    pub overlay: bool,
    pub overlay_mode: OverlayMode,
    pub overlay_position: OverlayCorner,
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
    /// The once-only look at the PCs' and games' own settings has run: before 4.1.1, pressing A
    /// on an inherited row pinned its value for the whole PC, and a pin can't be told apart from
    /// a choice (`Launcher::show_kept_settings_once`). Missing from files written before it.
    #[serde(default)]
    pub kept_settings_checked: bool,
    pub virtual_display: bool,
    /// "PC render resolution": the percent of the stream resolution the PC renders its virtual
    /// display at (`scaleFactor`), as on Android. Kept in `RENDER_SCALE` when read: older versions
    /// offered up to 300 %.
    #[serde(deserialize_with = "render_scale")]
    pub virtual_display_scale: u32,
    /// "PC display scaling": the Windows scale (text and interface size) of the virtual
    /// display, in percent (`rlDisplayScale`); 0 keeps the PC's own setting. A saved scale
    /// Windows does not offer reads as 0, as on Android: the PC would refuse the launch.
    #[serde(deserialize_with = "display_scale")]
    pub virtual_display_dpi: u32,
    pub host_display: HostDisplay,
    /// The video FEC asked for at launch from a PC that lets the client choose it.
    pub error_correction: ErrorCorrection,
    /// Link test before the first bitrate: a new stream at the automatic bitrate
    /// times `GET /bandwidth-probe` first and starts at no more than 80 % of it. Off by default:
    /// on the NUC it started streams lower with more loss and freezes (rust/PERFORMANCE.md).
    pub link_probe: bool,
    /// "Game optimization hint": the launch lets the PC adjust the game's settings for the stream
    /// (`sops`). On by default, as the launch always asked before it was a setting.
    pub sops: bool,
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
    /// The launcher's size in percent: one of [`UI_ZOOM_STEPS`]. Not a per-PC or per-game override.
    pub ui_zoom: u32,
    /// Brighter text, black surfaces and thicker outlines and focus rings.
    pub high_contrast: bool,
    /// "Theme" (Android's `ThemeChoice`, stored there as `checkbox_always_dark` and
    /// `checkbox_oled_black`): the system's light or dark, the night look, or OLED black.
    pub theme: Theme,
    pub reduce_motion: ReduceMotion,
    pub language: Language,
    /// Windows and AppImage: look for a newer stable release once per launch (update_check.rs).
    pub check_updates: bool,
    /// "Show decode time after streaming" (Android's `checkbox_enable_post_stream_toast`): when a
    /// stream ends, a notice gives its average decode time and codec. Off by default.
    pub post_stream_decode_time: bool,
}

/// The launcher's sizes in percent, smallest first (the same steps as Nightjar Android).
pub const UI_ZOOM_STEPS: [u32; 5] = [85, 100, 115, 130, 150];

/// The step to use for a stored size: the nearest one, so a hand-edited file still works.
pub fn nearest_zoom(percent: u32) -> u32 {
    UI_ZOOM_STEPS.into_iter().min_by_key(|step| step.abs_diff(percent)).unwrap_or(100)
}

/// The step after (`+1`) or before (`-1`) `percent`, kept inside the steps.
pub fn step_zoom(percent: u32, direction: i32) -> u32 {
    let at = UI_ZOOM_STEPS.iter().position(|s| *s == nearest_zoom(percent)).unwrap_or(1);
    let at = (at as i32 + direction).clamp(0, UI_ZOOM_STEPS.len() as i32 - 1);
    UI_ZOOM_STEPS[at as usize]
}

/// What the performance overlay shows while it is on, stored as Android's
/// `performance_overlay_mode` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    /// Six lines of stream measurements, with the host's CPU and GPU when it offers them.
    #[serde(alias = "expanded")]
    Statistics,
    /// Every measurement and decoder diagnostic, with each GPU's readings in full.
    #[serde(alias = "advanced")]
    Debug,
    /// One line: rendered FPS, network round trip and frame loss behind a health mark. Also
    /// what an unknown value reads as, as Android's `normalizeMode`; keep it last: serde wants
    /// the catch-all there.
    #[default]
    #[serde(other)]
    Compact,
}

impl OverlayMode {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Statistics, Self::Debug];

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => en("Compact"),
            Self::Statistics => en("Statistics"),
            Self::Debug => en("Debug"),
        }
    }
}

/// The corner the performance overlay sits in, stored as Android's
/// `performance_overlay_position` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayCorner {
    TopRight,
    BottomLeft,
    BottomRight,
    /// Also what an unknown value reads as; keep it last: serde wants the catch-all there.
    #[default]
    #[serde(other)]
    TopLeft,
}

impl OverlayCorner {
    pub const ALL: [Self; 4] = [Self::TopLeft, Self::TopRight, Self::BottomLeft, Self::BottomRight];

    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => en("Top left"),
            Self::TopRight => en("Top right"),
            Self::BottomLeft => en("Bottom left"),
            Self::BottomRight => en("Bottom right"),
        }
    }
}

/// How Home lays out its library rows (Android's `home_view_layout`).
/// "When Nightjar opens": the setting behind `nightjar_policy::open_play::Opens`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenOnStart {
    #[default]
    Pcs,
    Last,
    Chosen,
}

impl OpenOnStart {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pcs => en("Show my PCs"),
            Self::Last => en("Continue my last game"),
            Self::Chosen => en("A chosen game"),
        }
    }

    pub fn opens(self) -> nightjar_policy::open_play::Opens {
        use nightjar_policy::open_play::Opens;
        match self {
            Self::Pcs => Opens::Pcs,
            Self::Last => Opens::Last,
            Self::Chosen => Opens::Chosen,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeLayout {
    /// Every game of a row, line under line.
    Grid,
    /// Every game of a row as a line of its own: a small cover, the title, its store and when it
    /// was last played.
    List,
    /// One line per row, as many covers as fit and an "All N" tile for the rest. Also what an
    /// unknown value reads as; keep it last: serde wants the catch-all there.
    #[default]
    #[serde(other)]
    Rows,
}

impl HomeLayout {
    pub const ALL: [Self; 3] = [Self::Grid, Self::List, Self::Rows];

    pub fn label(self) -> &'static str {
        match self {
            Self::Grid => en("Grid"),
            Self::List => en("List"),
            Self::Rows => en("Rows"),
        }
    }
}

/// Whether Home splits its library by store, by type or by collection (Android's `home_view_group`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeGroup {
    /// The whole library as one row.
    None,
    /// The games of every store in one row, the apps of no store (launchers, the desktop) in
    /// their own.
    Type,
    /// A row per Steam collection or Playnite category the PCs name, a game in several in each,
    /// then the games in none. Offered while they name any.
    Collection,
    /// A row per store, then the apps of no store. Also what an unknown value reads as; keep it
    /// last: serde wants the catch-all there.
    #[default]
    #[serde(other)]
    Store,
}

impl HomeGroup {
    pub const ALL: [Self; 4] = [Self::Store, Self::Type, Self::Collection, Self::None];

    pub fn label(self) -> &'static str {
        match self {
            Self::Store => en("Store"),
            Self::Type => en("Type"),
            Self::Collection => en("Collection"),
            Self::None => en("None"),
        }
    }
}

/// The size of Home's covers (Android's `home_view_size`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardSize {
    Small,
    Large,
    /// Also what an unknown value reads as; keep it last: serde wants the catch-all there.
    #[default]
    #[serde(other)]
    Medium,
}

impl CardSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => en("Small"),
            Self::Medium => en("Medium"),
            Self::Large => en("Large"),
        }
    }

    /// A cover's widest against the medium one's, as on Android.
    pub fn scale(self) -> f32 {
        match self {
            Self::Small => 0.75,
            Self::Medium => 1.0,
            Self::Large => 1.3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReduceMotion {
    Reduce,
    Full,
    /// Reduce when the operating system asks for it (Windows: animations off). A value written
    /// by a newer Nightjar reads as this too, instead of dropping every setting. Keep it last:
    /// serde wants the catch-all there.
    #[default]
    #[serde(other)]
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    English,
    German,
    Spanish,
    French,
    Italian,
    Japanese,
    Korean,
    Polish,
    PortugueseBrazil,
    Russian,
    Turkish,
    Ukrainian,
    Vietnamese,
    ChineseSimplified,
    ChineseTraditional,
    /// The system's language when Nightjar has it, else English; also what an unknown value
    /// from a newer Nightjar reads as. Keep it last: serde wants the catch-all there.
    #[default]
    #[serde(other)]
    System,
}

impl Language {
    /// The choices in the settings, System default first.
    pub const ALL: [Language; 16] = [Language::System, Language::English, Language::German, Language::Spanish, Language::French, Language::Italian, Language::Japanese, Language::Korean, Language::Polish, Language::PortugueseBrazil, Language::Russian, Language::Turkish, Language::Ukrainian, Language::Vietnamese, Language::ChineseSimplified, Language::ChineseTraditional];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostDisplay {
    #[default]
    HostDefault,
    Exclusive,
    ExtendedPrimary,
    Extended,
    ExtendedIsolated,
    ExtendedPrimaryIsolated,
    Physical,
}

impl From<HostDisplay> for nightjar_core::http::HostDisplay {
    fn from(value: HostDisplay) -> Self {
        match value {
            HostDisplay::HostDefault => Self::HostDefault,
            HostDisplay::Exclusive => Self::Exclusive,
            HostDisplay::ExtendedPrimary => Self::ExtendedPrimary,
            HostDisplay::Extended => Self::Extended,
            HostDisplay::ExtendedIsolated => Self::ExtendedIsolated,
            HostDisplay::ExtendedPrimaryIsolated => Self::ExtendedPrimaryIsolated,
            HostDisplay::Physical => Self::Physical,
        }
    }
}

/// The video FEC this client asks for (`rlFecPercentage` from Rubylight, `x-ml-video.fecPercentage`
/// from Foundation Sunshine, at most 100 %): the PC's own, automatic from this device's loss
/// reports, or a fixed share. Other PCs keep their own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCorrection {
    #[default]
    Host,
    Auto,
    Percent(u8),
}

impl ErrorCorrection {
    /// The fixed shares the settings offer; a hand-edited file may hold any 0-255.
    pub const PERCENTAGES: [u8; 4] = [10, 20, 30, 50];
}

impl From<ErrorCorrection> for rubylight_protocol::fec_report::FecChoice {
    fn from(value: ErrorCorrection) -> Self {
        match value {
            ErrorCorrection::Host => Self::Host,
            ErrorCorrection::Auto => Self::Auto,
            ErrorCorrection::Percent(percentage) => Self::Fixed(percentage),
        }
    }
}

/// The PC render resolution's range in percent, as on Android (`seekbar_virtual_display_scale`).
pub const RENDER_SCALE: std::ops::RangeInclusive<u32> = 50..=200;

fn render_scale<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    Ok(u32::deserialize(deserializer)?.clamp(*RENDER_SCALE.start(), *RENDER_SCALE.end()))
}

/// The Windows scales "PC display scaling" offers in percent; 0 keeps the PC's own
/// (`rlDisplayScale`, Android's `VIRTUAL_DISPLAY_DPI_VALUES`).
pub const DISPLAY_SCALES: [u32; 13] = [0, 100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

fn display_scale<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let percent = u32::deserialize(deserializer)?;
    Ok(if DISPLAY_SCALES.contains(&percent) { percent } else { 0 })
}

/// The global stick dead zone's range in percent, as a controller's own on its buttons page.
pub const DEADZONE: std::ops::RangeInclusive<u32> = 0..=50;
/// The microphone gain's range in percent and its step, as on Android (`seekbar_mic_gain`).
pub const MIC_GAIN: std::ops::RangeInclusive<u32> = 25..=400;
pub const MIC_GAIN_STEP: u32 = 5;

impl Settings {
    /// What every load does to settings saved by an earlier version. Idempotent: it needs no flag
    /// but `auto_reconnect_default_on`'s.
    pub(crate) fn migrate(&mut self) {
        // Turns Recover interrupted streams on once in settings saved before it became the default.
        if !self.auto_reconnect_default_on {
            (self.auto_reconnect, self.auto_reconnect_default_on) = (true, true);
        }
        normalize_values(&mut SettingsValues(self));
    }
}

/// A settings layer's values, main or not, for [`normalize_values`].
trait Values {
    fn bool(&self, key: &str) -> Option<bool>;
    fn u32(&self, key: &str) -> Option<u32>;
    fn set_bool(&mut self, key: &str, value: bool);
    fn set_u32(&mut self, key: &str, value: u32);
}

/// The main settings, which hold every key.
struct SettingsValues<'a>(&'a mut Settings);

impl Values for SettingsValues<'_> {
    fn bool(&self, key: &str) -> Option<bool> {
        match key {
            "prefer_yuv444" => Some(self.0.prefer_yuv444),
            "experimental_packed_444" => Some(self.0.experimental_packed_444),
            _ => None,
        }
    }
    fn u32(&self, key: &str) -> Option<u32> {
        match key {
            "network_grace_ms" => Some(self.0.network_grace_ms),
            "quality_min_short_edge" => Some(self.0.quality_min_short_edge),
            "quality_min_fps" => Some(self.0.quality_min_fps),
            "mic_gain" => Some(self.0.mic_gain),
            "deadzone" => Some(self.0.deadzone),
            "trigger_deadzone" => Some(self.0.trigger_deadzone),
            _ => None,
        }
    }
    fn set_bool(&mut self, key: &str, value: bool) {
        match key {
            "prefer_yuv444" => self.0.prefer_yuv444 = value,
            "experimental_packed_444" => self.0.experimental_packed_444 = value,
            _ => {}
        }
    }
    fn set_u32(&mut self, key: &str, value: u32) {
        let s = &mut *self.0;
        match key {
            "network_grace_ms" => s.network_grace_ms = value,
            "quality_min_short_edge" => s.quality_min_short_edge = value,
            "quality_min_fps" => s.quality_min_fps = value,
            "mic_gain" => s.mic_gain = value,
            "deadzone" => s.deadzone = value,
            "trigger_deadzone" => s.trigger_deadzone = value,
            _ => {}
        }
    }
}

/// A PC's, game's or profile's own values, which hold only the keys they set.
impl Values for serde_json::Map<String, serde_json::Value> {
    fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(serde_json::Value::as_bool)
    }
    fn u32(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(serde_json::Value::as_u64).and_then(|v| u32::try_from(v).ok())
    }
    fn set_bool(&mut self, key: &str, value: bool) {
        self.insert(key.to_owned(), value.into());
    }
    fn set_u32(&mut self, key: &str, value: u32) {
        self.insert(key.to_owned(), value.into());
    }
}

/// The keys "Sharper colored text" is kept in (`nightjar_core::sharp_color`).
pub const SHARP_COLOR_KEYS: [&str; 2] = ["prefer_yuv444", "experimental_packed_444"];

/// Brings a layer's saved values onto what this version offers. Every load of every layer runs it
/// (the main settings, profiles, PCs and games), and it changes nothing a second time:
/// - "Sharper colored text" is one choice in two keys: a layer that holds either gets both, set to
///   whether either was on. A layer that holds neither keeps inheriting them.
/// - "Wait through network dropouts", "Minimum resolution" and "Minimum frame rate" read as the
///   nearest of the choices both clients offer (`nightjar_policy::recovery`, `::quality`).
/// - Microphone gain and the dead zones stay in their ranges.
fn normalize_values(values: &mut impl Values) {
    use nightjar_policy::quality::{nearest_floor, MIN_FPS_CHOICES, MIN_SHORT_EDGE_CHOICES};
    let held: Vec<bool> = SHARP_COLOR_KEYS.iter().filter_map(|key| values.bool(key)).collect();
    if !held.is_empty() {
        let on = held.contains(&true);
        for key in SHARP_COLOR_KEYS {
            values.set_bool(key, on);
        }
    }
    let fixes: [(&str, &dyn Fn(u32) -> u32); 6] = [
        ("network_grace_ms", &nightjar_policy::recovery::network_grace_ms),
        ("quality_min_short_edge", &|v: u32| nearest_floor(&MIN_SHORT_EDGE_CHOICES, v)),
        ("quality_min_fps", &|v: u32| nearest_floor(&MIN_FPS_CHOICES, v)),
        ("mic_gain", &|v: u32| v.clamp(*MIC_GAIN.start(), *MIC_GAIN.end()) / MIC_GAIN_STEP * MIC_GAIN_STEP),
        ("deadzone", &|v: u32| v.min(*DEADZONE.end())),
        ("trigger_deadzone", &|v: u32| v.min(u32::from(crate::gamepad::MAX_TRIGGER_DEADZONE))),
    ];
    for (key, fix) in fixes {
        if let Some(value) = values.u32(key) {
            let fixed = fix(value);
            if fixed != value {
                values.set_u32(key, fixed);
            }
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            fps: 0,
            bitrate_kbps: 0,
            adaptive_bitrate: false,
            client_hint_bitrate: false,
            adaptive_quality: QualityMode::Off,
            quality_min_short_edge: 720,
            quality_min_fps: 30,
            codec: CodecPreference::Auto,
            hdr: false,
            hdr_desktop_brightness: false,
            experimental_packed_444: false,
            prefer_yuv444: false,
            sdr_10bit: false,
            audio: AudioChannels::Stereo,
            play_audio_on_host: false,
            mute_audio: false,
            audio_passthrough: false,
            window_mode: WindowMode::Borderless,
            video_scaling: VideoScaling::Fit,
            align_video_top: false,
            vrr: true,
            present_at_host_times: false,
            vsync: true,
            frame_pacing: FramePacing::Latency,
            drop_late_frames: true,
            overlay: false,
            overlay_mode: OverlayMode::Compact,
            overlay_position: OverlayCorner::TopLeft,
            mouse_mode: MouseMode::Capture,
            mouse_speed: 100,
            mouse_nav_buttons: true,
            native_touchpad: false,
            right_alt_as_meta: false,
            capture_system_keys: true,
            system_keys_fullscreen_only: false,
            controllers: true,
            multi_controller: true,
            swap_face_buttons: false,
            controller_motion: true,
            controller_audio: false,
            back_opens_menu: false,
            back_as_guide: false,
            // 1 s, Android's default (`list_guide_menu_hold`); a lone press reaches the PC on release.
            guide_hold_ms: 1000,
            gamepad_quit_combo: GamepadQuitCombo::StartSelectLbRb,
            stream_menu_buttons: Default::default(),
            controller_mouse: true,
            analog_scrolling: AnalogScrolling::Right,
            mouse_fixed_step: false,
            gamepad_touchpad_as_mouse: false,
            deadzone: 0,
            trigger_deadzone: 0,
            disable_rumble: false,
            background_controllers: false,
            decoder: DecoderPreference::Auto,
            early_decode: false,
            beam_race: false,
            beam_race_margin_ms: 0,
            pyrowave_decoder: PyroWaveDecoder::default(),
            renderer: Renderer::Auto,
            graphics_card: GraphicsCard::Automatic,
            microphone: false,
            mic_echo_cancel: MicEcho::Auto,
            mic_gain: 100,
            mic_noise_suppression: true,
            mic_auto_gain: false,
            cursor_shapes: true,
            network_grace_ms: 3000,
            hide_connection_warnings: false,
            client_roaming: true,
            roaming_metered: false,
            receive_coalescing: true,
            usb_forwarding: false,
            usb_auto: Vec::new(),
            clipboard_sync: false,
            dual_screen: false,
            second_screen_width: 0,
            second_screen_height: 0,
            tell_host_wireless: true,
            tell_host_link_speed: true,
            unbatched_input: true,
            network_priority: true,
            software_slices: false,
            vulkan_video_first: false,
            vulkan_rfi: false,
            pyrowave_async_compute: false,
            auto_reconnect: true,
            auto_reconnect_default_on: true,
            kept_settings_checked: false,
            virtual_display: false,
            virtual_display_scale: 100,
            virtual_display_dpi: 0,
            host_display: HostDisplay::HostDefault,
            error_correction: ErrorCorrection::Host,
            link_probe: false,
            sops: true,
            full_range: false,
            upscaling: Upscaling::Bilinear,
            upscale_sharpness: 50,
            frame_warp: false,
            open_on_start: OpenOnStart::Pcs,
            console_home: true,
            quick_resume: false,
            input_only_any_host: false,
            home_layout: HomeLayout::Rows,
            home_group: HomeGroup::Store,
            home_card_size: CardSize::Medium,
            ui_zoom: 100,
            high_contrast: false,
            theme: Theme::Dark,
            reduce_motion: ReduceMotion::System,
            language: Language::System,
            check_updates: true,
            post_stream_decode_time: false,
        }
    }
}

impl Settings {
    /// Whether these settings draw through the GPU renderer, which Linux picks at start: SDL's
    /// "gpu" renderer runs the shader passes (packed full chroma, HDR, PyroWave, upscaling, and
    /// the plane pass that 4:4:4 and 10-bit pictures need).
    #[cfg(any(not(target_os = "macos"), test))]
    pub fn needs_gpu(&self) -> bool {
        self.gpu_need().is_some()
    }

    /// What in these settings needs the GPU renderer, for the log of a renderer switch; HDR comes
    /// last, so "HDR" means nothing else does.
    pub fn gpu_need(&self) -> Option<&'static str> {
        [
            (self.experimental_packed_444, "packed full chroma"),
            (self.codec.is_pyrowave(), en("PyroWave")),
            (self.upscaling != Upscaling::Bilinear, "upscaling"),
            (self.prefer_yuv444, "4:4:4"),
            (self.sdr_10bit, "10-bit SDR"),
            (self.hdr, en("HDR")),
        ]
        .into_iter()
        .find_map(|(on, why)| on.then_some(why))
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub unique_id: String,
    pub certificate_pem: String,
    pub private_key_pem: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostRecord {
    pub pace: Option<PaceCalibration>,
    /// The host's `uniqueid`; empty until the host answered once.
    pub unique_id: String,
    pub name: String,
    /// A name chosen on this device. The PC's own `name` keeps following what it reports.
    pub alias: Option<String>,
    /// Last address that answered, then the others it was known by.
    pub address: String,
    pub http_port: u16,
    pub https_port: u16,
    pub other_addresses: Vec<String>,
    /// Other HTTP ports at its addresses that answered with this PC's unique ID, from a listing
    /// of it there that was merged into this one: a sighting at one of them is this PC.
    pub other_http_ports: Vec<u16>,
    /// The known address the user chose to always connect through, or None for the automatic
    /// pick (`address` first, then `other_addresses`), as Moonlight V+ PC's `pinnedAddress`.
    pub pinned_address: Option<String>,
    /// The WAN address that reaches the PC from outside its network, tried after the others
    /// (Moonlight's remote address): the PC's own `ExternalIP`, else this device's STUN answer
    /// when the PC was found on the LAN.
    pub remote_address: Option<String>,
    /// The certificate pinned at pairing (PEM); None while unpaired.
    pub server_certificate: Option<String>,
    pub mac: Option<String>,
    /// The MACs of the PC's other adapters and the subnet broadcasts that reach them, from its
    /// native `GET /wake` (`wol::targets`); kept so a PC that sleeps can be woken through them.
    pub wake_macs: Vec<String>,
    pub wake_broadcasts: Vec<String>,
    /// MACs the user added for the PC: an adapter that wakes it while the one paired over cannot
    /// (`wol::parse_macs`). What the PC lists never replaces them.
    pub own_wake_macs: Vec<String>,
    /// Added by hand rather than found on the network.
    pub manual: bool,
    pub settings: Option<Overrides>,
    pub app_settings: BTreeMap<String, Overrides>,
    /// The games whose stream quits them on the PC when the player leaves it, kept as Android's
    /// `AppQuitOnLeave` does: under `uuid:<uuid>` and `id:<id>`.
    pub quit_on_leave: BTreeSet<String>,
    /// The once-per-PC notices already shown for this PC, by Android's `HostNotices` names.
    pub notices: BTreeSet<String>,
    /// What this PC's answers to full chroma left: its last offered modes and whether it refused
    /// the last stream that asked (Android's `HostPackedModes`).
    pub full_chroma: nightjar_core::full_chroma::Memory,
}

/// PyroWave was chosen and a stream from this PC ran another codec.
pub const NOTICE_PYROWAVE_FALLBACK: &str = "PYROWAVE_FALLBACK";
/// Full chroma is on and a stream from this PC starts without it: the PC or this device cannot do it.
pub const NOTICE_PACKED_UNAVAILABLE: &str = "PACKED_UNAVAILABLE";
/// Frame Warp is on and a stream from this PC runs without it: the PC does not offer it.
pub const NOTICE_FRAME_WARP_UNAVAILABLE: &str = "FRAME_WARP_UNAVAILABLE";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaceCalibration {
    pub pace_mbps: u32,
    pub client_link_mbps: u32,
    pub host_link_mbps: u32,
    pub wifi: bool,
    pub ssid: Option<String>,
    pub qualified: bool,
}

impl PaceCalibration {
    pub fn for_network(&self, network: &crate::network::Network, host_link: u32) -> u32 {
        if network.error.is_none() && self.client_link_mbps == network.link_mbps && self.host_link_mbps == host_link
            && self.wifi == network.wifi && (!self.wifi || (self.ssid.is_some() && self.ssid == network.ssid))
            && (1..=9999).contains(&self.pace_mbps) { self.pace_mbps } else { 0 }
    }
}

/// The longest name a PC can be given on this device.
pub const ALIAS_MAX_CHARS: usize = 40;

impl HostRecord {
    /// Whether there is a MAC to wake the PC by: serverinfo's, one its `/wake` listed, or one the
    /// user added. Every wake gate asks this.
    pub fn can_wake(&self) -> bool {
        self.mac.is_some() || !self.wake_macs.is_empty() || !self.own_wake_macs.is_empty()
    }

    /// Keeps `text` as this device's name for the PC. Empty (or blank) restores the PC's own name.
    pub fn set_alias(&mut self, text: &str) {
        let text: String = text.trim().chars().take(ALIAS_MAX_CHARS).collect();
        self.alias = (!text.is_empty()).then_some(text);
    }

    pub fn settings(&self, global: &Settings) -> Settings {
        self.settings.as_ref().map_or_else(|| global.clone(), |own| own.apply(global))
    }

    /// True the first time `notice` is shown for this PC, and remembered (Android's `HostNotices.claim`).
    pub fn claim_notice(&mut self, notice: &str) -> bool {
        self.notices.insert(notice.to_owned())
    }

    /// The PC has the feature again: if it loses it later, the notice is shown again. True when it was shown.
    pub fn reset_notice(&mut self, notice: &str) -> bool {
        self.notices.remove(notice)
    }

    /// What this PC keeps over the main settings and profiles: whether it has settings of its
    /// own, and for how many games (each saved under `id:`, and `uuid:` when it has one).
    pub fn own_settings(&self) -> (bool, usize) {
        let count = |prefix: &str| self.app_settings.iter().filter(|(key, own)| key.starts_with(prefix) && !own.is_empty()).count();
        (self.settings.as_ref().is_some_and(|own| !own.is_empty()), count("id:").max(count("uuid:")))
    }

    /// Drops the PC's and its games' own settings, so they follow the profile or main settings.
    pub fn clear_own_settings(&mut self) {
        self.settings = None;
        self.app_settings.clear();
    }

    pub fn app_settings(&self, uuid: &str, id: i32) -> Option<&Overrides> {
        self.app_settings.get(&app_key(uuid, id))
    }

    pub fn save_app_settings(&mut self, uuid: &str, id: i32, settings: Overrides) {
        let mut keys = vec![app_key(uuid, id)];
        if !uuid.is_empty() { keys.push(app_key("", id)); }
        for key in keys {
            if settings.is_empty() { self.app_settings.remove(&key); }
            else { self.app_settings.insert(key, settings.clone()); }
        }
    }

    /// Whether leaving this game's stream quits it. An ID can be reused for another game on hosts
    /// with UUIDs, so a known UUID decides alone.
    pub fn quits_on_leave(&self, uuid: &str, id: i32) -> bool {
        if !uuid.is_empty() { return self.quit_on_leave.contains(&app_key(uuid, 0)); }
        id > 0 && self.quit_on_leave.contains(&app_key("", id))
    }

    pub fn set_quit_on_leave(&mut self, uuid: &str, id: i32, enabled: bool) {
        let mut keys = Vec::new();
        if !uuid.is_empty() { keys.push(app_key(uuid, id)); }
        if id > 0 { keys.push(app_key("", id)); }
        for key in keys {
            if enabled { self.quit_on_leave.insert(key); } else { self.quit_on_leave.remove(&key); }
        }
    }
}

/// The layer a stream setting's value comes from, from the top: the game, the PC, the profile in
/// use, else the main settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Main,
    Profile(String),
    Pc,
    Game,
}

pub fn source(keys: &[&str], profile: Option<&Profile>, pc: Option<&Overrides>, game: Option<&Overrides>) -> Source {
    if game.is_some_and(|o| o.has(keys)) {
        Source::Game
    } else if pc.is_some_and(|o| o.has(keys)) {
        Source::Pc
    } else {
        profile.filter(|p| p.settings.has(keys)).map_or(Source::Main, |p| Source::Profile(p.name.clone()))
    }
}

fn app_key(uuid: &str, id: i32) -> String {
    if uuid.is_empty() { format!("id:{id}") } else { format!("uuid:{uuid}") }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Overrides(serde_json::Map<String, serde_json::Value>);

impl<'de> Deserialize<'de> for Overrides {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut values = serde_json::Map::deserialize(deserializer)?;
        serde_json::from_value::<Settings>(serde_json::Value::Object(values.clone())).map_err(serde::de::Error::custom)?;
        normalize_values(&mut values);
        Ok(Self(values))
    }
}

impl Overrides {
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    pub fn has(&self, keys: &[&str]) -> bool { keys.iter().any(|key| self.0.contains_key(*key)) }

    pub fn capture(&mut self, settings: &Settings, keys: &[&str]) {
        let values = serde_json::to_value(settings).expect("settings serialize");
        for key in keys { self.0.insert((*key).to_owned(), values[*key].clone()); }
    }

    pub fn clear(&mut self, keys: &[&str]) {
        for key in keys { self.0.remove(*key); }
    }

    pub fn apply(&self, base: &Settings) -> Settings {
        let mut values = serde_json::to_value(base).expect("settings serialize");
        values.as_object_mut().unwrap().extend(self.0.clone());
        serde_json::from_value(values).expect("validated settings overrides")
    }

    #[cfg(any(not(target_os = "macos"), test))]
    pub fn needs_gpu(&self) -> bool {
        self.apply(&Settings::default()).needs_gpu()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: u64,
    pub name: String,
    pub settings: Overrides,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profiles {
    pub active: Option<u64>,
    pub entries: Vec<Profile>,
    pub automatic: bool,
    pub mobile: Option<u64>,
    pub home: Option<u64>,
    pub wifi: Option<u64>,
    pub home_wifi: Vec<String>,
    pub mobile_limit_kbps: u32,
}

impl Profiles {
    pub fn selected(&self, network: crate::network::Kind) -> Option<u64> {
        let rule = if self.automatic {
            match network { crate::network::Kind::Mobile => self.mobile, crate::network::Kind::HomeWifi => self.home,
                crate::network::Kind::Wifi => self.wifi, crate::network::Kind::Other => None }
        } else { None };
        rule.or(self.active)
    }

    pub fn settings(&self, base: &Settings, network: crate::network::Kind) -> Settings {
        self.profile(network).map_or_else(|| base.clone(), |p| p.settings.apply(base))
    }

    /// The profile a stream on `network` uses, None for the main settings.
    pub fn profile(&self, network: crate::network::Kind) -> Option<&Profile> {
        self.entries.iter().find(|p| Some(p.id) == self.selected(network))
    }

    pub fn bitrate_limit(&self, network: crate::network::Kind) -> u32 {
        if network == crate::network::Kind::Mobile { self.mobile_limit_kbps } else { 0 }
    }

    pub fn check_name(&self, name: &str, except: Option<u64>) -> Result<(), &'static str> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 32 { return Err(en("Use a name between 1 and 32 characters.")); }
        if self.entries.iter().any(|p| Some(p.id) != except && p.name.to_lowercase() == name.to_lowercase()) {
            return Err(en("A profile already uses that name."));
        }
        Ok(())
    }

    pub fn create(&mut self, name: &str, settings: Overrides) -> Result<u64, &'static str> {
        self.check_name(name, None)?;
        if self.entries.len() >= 50 { return Err(en("You can save up to 50 profiles.")); }
        let id = self.entries.iter().map(|p| p.id).max().unwrap_or(0).checked_add(1).ok_or(en("No profile IDs available."))?;
        self.entries.push(Profile { id, name: name.trim().to_owned(), settings });
        Ok(id)
    }

    pub fn delete(&mut self, id: u64) {
        self.entries.retain(|p| p.id != id);
        if self.active == Some(id) { self.active = None; }
        for rule in [&mut self.mobile, &mut self.home, &mut self.wifi] {
            if *rule == Some(id) { *rule = None; }
        }
    }
}

pub struct Store {
    dir: PathBuf,
    /// The second screen's process (`second_screen.rs`) reads the files and leaves them as the
    /// first screen's process writes them.
    read_only: bool,
}

/// A game that was streamed, for Home's "Recently played". It names the PC by its unique id and
/// the game by the install id the PC gives both of its lists (else the app id), so it still
/// finds the game when the PC has a new address or the library is reordered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    pub host: String,
    pub install_id: String,
    pub app_id: u32,
    pub title: String,
    /// Seconds since the Unix epoch.
    pub at: u64,
}

/// The game a start of Nightjar goes on to, named like a [`Recent`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenGame {
    pub host: String,
    pub install_id: String,
    pub app_id: u32,
    pub title: String,
}

impl OpenGame {
    pub fn same_game(&self, other: &OpenGame) -> bool {
        self.host == other.host
            && if !self.install_id.is_empty() && !other.install_id.is_empty() {
                self.install_id.eq_ignore_ascii_case(&other.install_id)
            } else {
                self.app_id == other.app_id
            }
    }
}

/// What the automatic start keeps between runs (`open_play.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenPlayState {
    /// The game chosen in its menu.
    pub game: Option<OpenGame>,
    /// An automatic start began and has not ended well yet (`nightjar_policy::open_play`).
    pub armed: bool,
}

/// How many recent games are kept.
pub const RECENT_LIMIT: usize = 12;

impl Recent {
    fn same_game(&self, other: &Recent) -> bool {
        self.host == other.host
            && if !self.install_id.is_empty() && !other.install_id.is_empty() {
                self.install_id.eq_ignore_ascii_case(&other.install_id)
            } else {
                self.app_id == other.app_id
            }
    }
}

/// Puts `entry` first; the same game earlier in the list is replaced, and the list is capped.
pub fn remember(list: &mut Vec<Recent>, entry: Recent) {
    list.retain(|r| !r.same_game(&entry));
    list.insert(0, entry);
    list.truncate(RECENT_LIMIT);
}

/// A game the player starred, for Home's "Favourites". It is keyed like a [`Recent`]: the PC's
/// unique id and the game's install id (else its app id), so the same title on two PCs is two
/// favourites and a new address or a reordered library does not lose it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favourite {
    pub host: String,
    pub install_id: String,
    pub app_id: u32,
    /// Shown while the PC's library is not known (it is offline and nothing is saved).
    pub title: String,
}

impl Favourite {
    pub fn same_game(&self, other: &Favourite) -> bool {
        self.host == other.host
            && if !self.install_id.is_empty() && !other.install_id.is_empty() {
                self.install_id.eq_ignore_ascii_case(&other.install_id)
            } else {
                self.app_id == other.app_id
            }
    }
}

impl Favourite {
    /// A hidden game by either id: the PC's list knows it by its classic UUID and Home by its
    /// install id, and the app id is the same in both.
    pub fn same_hidden(&self, other: &Favourite) -> bool {
        self.same_game(other) || (self.host == other.host && self.app_id == other.app_id)
    }
}

/// Hides the `games` or shows them again, by the keys a favourite has. Returns whether the list
/// changed.
pub fn set_hidden(list: &mut Vec<Favourite>, games: Vec<Favourite>, hidden: bool) -> bool {
    let before = list.len();
    list.retain(|h| !games.iter().any(|game| game.same_hidden(h)));
    let dropped = before - list.len();
    if !hidden {
        return dropped > 0;
    }
    let added = games.len();
    list.extend(games);
    added != dropped
}

/// Stars `entry` (first in the list; a game already starred moves up and takes the new title)
/// or, when it is starred already, removes it. Returns whether the game is starred now.
pub fn toggle_favourite(list: &mut Vec<Favourite>, entry: Favourite) -> bool {
    let before = list.len();
    list.retain(|f| !f.same_game(&entry));
    if list.len() < before {
        return false;
    }
    list.insert(0, entry);
    true
}

/// The starred games in `text`, skipping entries that do not parse, and whether anything was
/// skipped (or the whole text was unreadable), so the caller can keep the original aside.
fn parse_favourites(text: &str) -> (Vec<Favourite>, bool) {
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(text) else {
        return (Vec::new(), true);
    };
    let total = entries.len();
    let list: Vec<Favourite> = entries.into_iter().filter_map(|e| serde_json::from_value(e).ok()).collect();
    let damaged = list.len() < total;
    (list, damaged)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(e) => {
            eprintln!("nightjar: ignoring {}: {e}", path.display());
            None
        }
    }
}

/// Removes from a settings object the values this version can't read (one a newer version wrote,
/// or a hand edit), so one of them doesn't cost the others. Anything but an object becomes empty.
fn keep_readable_settings(values: &mut serde_json::Value) {
    let Some(map) = values.as_object_mut() else {
        *values = serde_json::Value::Object(serde_json::Map::new());
        return;
    };
    map.retain(|key, value| {
        let one = serde_json::Map::from_iter([(key.clone(), value.clone())]);
        let readable = serde_json::from_value::<Settings>(serde_json::Value::Object(one)).is_ok();
        if !readable {
            eprintln!("nightjar: leaving out the setting {key}: this version can't read {value}");
        }
        readable
    });
}

/// profiles.json with each profile's unreadable settings left out.
fn keep_readable_profiles(profiles: &mut serde_json::Value) {
    let entries = profiles.get_mut("entries").and_then(serde_json::Value::as_array_mut);
    for settings in entries.into_iter().flatten().filter_map(|profile| profile.get_mut("settings")) {
        keep_readable_settings(settings);
    }
}

/// hosts.json with each PC's and game's unreadable settings left out, then the PCs that still
/// can't be read: the others keep their pairing.
fn keep_readable_hosts(hosts: &mut serde_json::Value) {
    let Some(hosts) = hosts.as_array_mut() else { return };
    for host in hosts.iter_mut() {
        if let Some(settings) = host.get_mut("settings").filter(|settings| !settings.is_null()) {
            keep_readable_settings(settings);
        }
        match host.get_mut("app_settings") {
            Some(serde_json::Value::Object(games)) => games.values_mut().for_each(keep_readable_settings),
            Some(other) => *other = serde_json::Value::Object(serde_json::Map::new()),
            None => {}
        }
    }
    hosts.retain(|host| serde_json::from_value::<HostRecord>(host.clone()).is_ok());
}

const HOTKEYS_FILE: &str = "stream_hotkeys.json";
/// stream_hotkeys.json's format: `{"version": 1, "bindings": {"quit": "CTRL+F9", "overlay": "off"}}`.
const HOTKEYS_VERSION: u64 = 1;

/// stream_hotkeys.json as read: the bindings this version reads, and the entries it doesn't (not a
/// string), kept for the next save.
struct HotkeysFile {
    version: u64,
    bindings: nightjar_policy::shortcuts::HotkeyBindings,
    kept: serde_json::Map<String, serde_json::Value>,
}

/// None for a missing file; an error for one that can't be read or isn't the format's shape.
fn read_hotkeys(path: &Path) -> Result<Option<HotkeysFile>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let version = value.get("version").and_then(serde_json::Value::as_u64).ok_or("no version")?;
    let entries = value.get("bindings").and_then(serde_json::Value::as_object).ok_or("no bindings")?;
    let mut file = HotkeysFile { version, bindings: Default::default(), kept: serde_json::Map::new() };
    for (id, entry) in entries {
        match entry.as_str() {
            Some(text) => { file.bindings.insert(id.clone(), text.to_string()); }
            None => { file.kept.insert(id.clone(), entry.clone()); }
        }
    }
    Ok(Some(file))
}

/// Copies `path` to `<path>.<suffix>` (`.<suffix>.2` and on beside earlier, different copies),
/// unless a copy already holds the same bytes.
fn back_up(path: &Path, suffix: &str) -> std::io::Result<()> {
    let bytes = std::fs::read(path)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    for n in 1.. {
        let copy = path.with_file_name(if n == 1 { format!("{name}.{suffix}") } else { format!("{name}.{suffix}.{n}") });
        match std::fs::read(&copy) {
            Ok(earlier) if earlier == bytes => return Ok(()),
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                write_private(&copy, &bytes)?;
                eprintln!("nightjar: copied {} to {} before replacing it", path.display(), copy.display());
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("a free name")
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    write_private(path, text.as_bytes())
}

/// Writes `bytes` to `path` whole or not at all: a temporary file of its own name, synced to disk,
/// then renamed over `path`, so neither a crash nor a second writer leaves half a file. On Unix only
/// the user can read it (0600): the identity holds the client's private key, the PIN lock its hash.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?.to_string_lossy();
    let temporary = path.with_file_name(format!("{name}.{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        // The mode above is masked by the umask; this sets it as asked.
        #[cfg(unix)]
        file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// The platform config directory, or `NIGHTJAR_CONFIG_DIR`. Not created here.
pub fn config_dir() -> std::io::Result<PathBuf> {
    match std::env::var_os("NIGHTJAR_CONFIG_DIR") {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => directories::ProjectDirs::from("", "", "Nightjar")
            .map(|d| d.config_dir().to_path_buf())
            .ok_or_else(|| std::io::Error::other("no config directory")),
    }
}

impl Store {
    /// The platform config directory, or `NIGHTJAR_CONFIG_DIR` (used by tests and the
    /// end-to-end harness).
    pub fn open() -> std::io::Result<Self> {
        Self::at(config_dir()?)
    }

    pub fn at(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir, read_only: false })
    }

    /// The same files, read and never written or removed: every save does nothing.
    pub fn read_only(self) -> Self {
        Self { read_only: true, ..self }
    }

    fn write<T: Serialize>(&self, path: &Path, value: &T) -> std::io::Result<()> {
        if self.read_only {
            return Ok(());
        }
        write_json(path, value)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads `name`, or the default when there is none. A file this version can't read whole is
    /// first copied to `<name>.bad`, because the next save replaces it; then it is read again
    /// with what `keep_readable` left of it, so one unreadable value doesn't cost the rest.
    fn read_or_keep_readable<T: Default + for<'de> Deserialize<'de>>(&self, name: &str, keep_readable: fn(&mut serde_json::Value)) -> T {
        let path = self.dir.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return T::default();
        };
        let error = match serde_json::from_str(&text) {
            Ok(value) => return value,
            Err(error) => error,
        };
        eprintln!("nightjar: {} can't be read whole: {error}", path.display());
        if !self.read_only {
            if let Err(e) = back_up(&path, "bad") {
                eprintln!("nightjar: could not keep a copy of {}: {e}", path.display());
            }
        }
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return T::default();
        };
        keep_readable(&mut value);
        serde_json::from_value(value).unwrap_or_else(|e| {
            eprintln!("nightjar: ignoring {}: {e}", path.display());
            T::default()
        })
    }

    pub fn settings(&self) -> Settings {
        let mut settings: Settings = self.read_or_keep_readable("settings.json", keep_readable_settings);
        settings.migrate();
        settings
    }

    pub fn save_settings(&self, settings: &Settings) -> std::io::Result<()> {
        self.write(&self.dir.join("settings.json"), settings)
    }

    pub fn profiles(&self) -> Profiles {
        self.read_or_keep_readable("profiles.json", keep_readable_profiles)
    }

    pub fn save_profiles(&self, profiles: &Profiles) -> std::io::Result<()> {
        self.write(&self.dir.join("profiles.json"), profiles)
    }

    pub fn controller_mappings(&self) -> BTreeMap<String, crate::gamepad::Mapping> {
        let mut mappings: BTreeMap<String, crate::gamepad::Mapping> = self.read_or_keep_readable("controllers.json", |_| {});
        mappings.retain(|_, mapping| mapping.valid());
        mappings
    }

    pub fn shortcuts(&self) -> crate::shortcuts::Shortcuts {
        let path = self.dir.join("shortcuts.json");
        if !path.exists() { return crate::shortcuts::Shortcuts::default(); }
        crate::shortcuts::read(&path).unwrap_or_else(|e| {
            eprintln!("nightjar: ignoring {}: {e}", path.display());
            crate::shortcuts::Shortcuts::default()
        })
    }

    pub fn window_placement(&self) -> Option<crate::window_place::Placement> {
        read_json(&self.dir.join("window.json"))
    }

    pub fn save_window_placement(&self, placement: &crate::window_place::Placement) -> std::io::Result<()> {
        self.write(&self.dir.join("window.json"), placement)
    }

    pub fn save_shortcuts(&self, shortcuts: &crate::shortcuts::Shortcuts) -> std::io::Result<()> {
        self.write(&self.dir.join("shortcuts.json"), shortcuts)
    }

    /// The user's changes to the stream hotkeys, by action ID (nightjar-policy's `HotkeyBindings`),
    /// read entry by entry: an entry this version can't read is logged and kept in the file. A
    /// missing file, or one that doesn't read at all (logged), leaves every hotkey at its default.
    pub fn hotkeys(&self) -> nightjar_policy::shortcuts::HotkeyBindings {
        let path = self.dir.join(HOTKEYS_FILE);
        match read_hotkeys(&path) {
            Ok(Some(file)) => {
                if file.version > HOTKEYS_VERSION {
                    eprintln!("nightjar: {} is from a newer Nightjar (version {}); reading the entries this one knows", path.display(), file.version);
                }
                for id in file.kept.keys() {
                    eprintln!("nightjar: keeping {id} in {} unread: not a chord this version reads", path.display());
                }
                file.bindings
            }
            Ok(None) => Default::default(),
            Err(e) => {
                eprintln!("nightjar: cannot read {}: {e}; the stream hotkeys keep their defaults, and the file is backed up before a change replaces it", path.display());
                Default::default()
            }
        }
    }

    /// Saves the stream hotkeys over stream_hotkeys.json, keeping the entries it holds that this
    /// version doesn't read and the newer version that wrote it. A file that doesn't read, or one a
    /// newer Nightjar wrote, is copied beside it first (`.unreadable`, `.v<version>`), never lost.
    pub fn save_hotkeys(&self, bindings: &nightjar_policy::shortcuts::HotkeyBindings) -> std::io::Result<()> {
        let path = self.dir.join(HOTKEYS_FILE);
        let (version, mut entries) = match read_hotkeys(&path) {
            Ok(Some(file)) => {
                if file.version > HOTKEYS_VERSION && !self.read_only {
                    back_up(&path, &format!("v{}", file.version))?;
                }
                (file.version.max(HOTKEYS_VERSION), file.kept)
            }
            Ok(None) => (HOTKEYS_VERSION, serde_json::Map::new()),
            Err(_) if self.read_only => (HOTKEYS_VERSION, serde_json::Map::new()),
            Err(_) => {
                back_up(&path, "unreadable")?;
                (HOTKEYS_VERSION, serde_json::Map::new())
            }
        };
        for (id, text) in bindings {
            entries.insert(id.clone(), serde_json::Value::String(text.clone()));
        }
        self.write(&path, &serde_json::json!({"version": version, "bindings": entries}))
    }

    pub fn save_controller_mappings(&self, mappings: &BTreeMap<String, crate::gamepad::Mapping>) -> std::io::Result<()> {
        self.write(&self.dir.join("controllers.json"), mappings)
    }

    pub fn identity(&self) -> Option<Identity> {
        read_json(&self.dir.join("identity.json"))
    }

    pub fn save_identity(&self, identity: &Identity) -> std::io::Result<()> {
        self.write(&self.dir.join("identity.json"), identity)
    }

    pub fn hosts(&self) -> Vec<HostRecord> {
        self.read_or_keep_readable("hosts.json", keep_readable_hosts)
    }

    pub fn save_hosts(&self, hosts: &[HostRecord]) -> std::io::Result<()> {
        self.write(&self.dir.join("hosts.json"), &hosts)
    }

    pub fn recents(&self) -> Vec<Recent> {
        read_json(&self.dir.join("recent.json")).unwrap_or_default()
    }

    pub fn save_recents(&self, recents: &[Recent]) -> std::io::Result<()> {
        self.write(&self.dir.join("recent.json"), &recents)
    }

    pub fn open_play(&self) -> OpenPlayState {
        read_json(&self.dir.join("open_play.json")).unwrap_or_default()
    }

    pub fn save_open_play(&self, state: &OpenPlayState) -> std::io::Result<()> {
        self.write(&self.dir.join("open_play.json"), state)
    }

    pub fn favourites(&self) -> Vec<Favourite> {
        let path = self.dir.join("favourites.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let (list, damaged) = parse_favourites(&text);
        if damaged {
            // Keep the list as it was, so a bad entry (or a file the next save would replace) is not lost.
            eprintln!("nightjar: favourites.json has entries that could not be read; kept as favourites.json.bak");
            let _ = std::fs::copy(&path, path.with_extension("json.bak"));
        }
        list
    }

    pub fn save_favourites(&self, favourites: &[Favourite]) -> std::io::Result<()> {
        self.write(&self.dir.join("favourites.json"), &favourites)
    }

    /// The games hidden on this device, like the favourites' file; no other device reads it.
    pub fn hidden_games(&self) -> Vec<Favourite> {
        let path = self.dir.join("hidden-games.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let (list, damaged) = parse_favourites(&text);
        if damaged {
            eprintln!("nightjar: hidden-games.json has entries that could not be read; kept as hidden-games.json.bak");
            let _ = std::fs::copy(&path, path.with_extension("json.bak"));
        }
        list
    }

    pub fn save_hidden_games(&self, games: &[Favourite]) -> std::io::Result<()> {
        self.write(&self.dir.join("hidden-games.json"), &games)
    }

    /// Box art cache: `art/<host>/<app>.img`, or `<app>-<version>.img` for a cover the host
    /// versions (the native API), so a changed cover is fetched again.
    pub fn art_path(&self, host: &str, app_id: i32, version: &str) -> PathBuf {
        let version: String = version.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let name = if version.is_empty() { format!("{app_id}.img") } else { format!("{app_id}-{version}.img") };
        self.dir.join("art").join(sanitize(host)).join(name)
    }

    /// The last native library a PC sent, for Home while the PC is away or not polled yet.
    pub fn library(&self, host: &str) -> Option<NativeLibrary> {
        read_json(&self.library_path(host))
    }

    pub fn save_library(&self, host: &str, library: &NativeLibrary) -> std::io::Result<()> {
        let path = self.library_path(host);
        if self.read_only {
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        self.write(&path, library)
    }

    /// The last app list a PC without the native library sent, for Home while the PC is away or
    /// not polled yet (Android keeps the same per PC). None when nothing usable is saved.
    pub fn classic_apps(&self, host: &str) -> Option<ClassicApps> {
        read_json::<ClassicApps>(&self.classic_apps_path(host))
    }

    pub fn save_classic_apps(&self, host: &str, apps: &ClassicApps) -> std::io::Result<()> {
        let path = self.classic_apps_path(host);
        if self.read_only {
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        self.write(&path, apps)
    }

    pub fn forget_classic_apps(&self, host: &str) {
        if !self.read_only {
            let _ = std::fs::remove_file(self.classic_apps_path(host));
        }
    }

    /// The saved native library and app list of a PC.
    pub fn forget_library(&self, host: &str) {
        if !self.read_only {
            let _ = std::fs::remove_file(self.library_path(host));
        }
        self.forget_classic_apps(host);
    }

    /// Everything kept for a PC that is removed: its library, its covers, and its games in the
    /// recent and favourite lists. Returns both lists as they are now.
    pub fn forget_host(&self, host: &str, recents: &[Recent], favourites: &[Favourite]) -> (Vec<Recent>, Vec<Favourite>) {
        self.forget_library(host);
        if !self.read_only {
            let _ = std::fs::remove_dir_all(self.dir.join("art").join(sanitize(host)));
        }
        let mut open_play = self.open_play();
        if open_play.game.as_ref().is_some_and(|g| g.host == host) {
            open_play.game = None;
            if let Err(e) = self.save_open_play(&open_play) {
                eprintln!("nightjar: saving the game to open failed: {e}");
            }
        }
        let kept: Vec<Recent> = recents.iter().filter(|r| r.host != host).cloned().collect();
        if kept.len() != recents.len() {
            if let Err(e) = self.save_recents(&kept) {
                eprintln!("nightjar: saving recent games failed: {e}");
            }
        }
        let stars: Vec<Favourite> = favourites.iter().filter(|f| f.host != host).cloned().collect();
        if stars.len() != favourites.len() {
            if let Err(e) = self.save_favourites(&stars) {
                eprintln!("nightjar: saving favourites failed: {e}");
            }
        }
        (kept, stars)
    }

    fn library_path(&self, host: &str) -> PathBuf {
        self.dir.join("library").join(format!("{}.json", sanitize(host)))
    }

    fn classic_apps_path(&self, host: &str) -> PathBuf {
        self.dir.join("apps").join(format!("{}.json", sanitize(host)))
    }
}

/// The version of the saved app list's format. A later format that only adds fields keeps it;
/// fields this build does not know are ignored, and missing ones take their defaults.
pub const CLASSIC_APPS_VERSION: u32 = 1;

/// A PC's app list as saved for Home: its games, without the control tiles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicApps {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub apps: Vec<ClassicApp>,
}

/// One game of a saved app list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicApp {
    pub id: i32,
    #[serde(default)]
    pub name: String,
    /// The host's UUID for the app; empty when it sends none.
    #[serde(default)]
    pub uuid: String,
    /// The box art's version, empty when the host sends none.
    #[serde(default)]
    pub art_version: String,
}

impl ClassicApps {
    /// The games of a loaded app list, in its order: the control tiles (Resume, Disconnect,
    /// Terminate, Remote Input...) are not games and stay on the PC's own screen.
    pub fn of(apps: &[nightjar_core::http::App]) -> Self {
        let apps = apps
            .iter()
            .filter(|a| a.control == nightjar_core::http::AppControl::None)
            .map(|a| ClassicApp { id: a.id, name: a.name.clone(), uuid: a.uuid.clone(), art_version: a.art_version.clone() })
            .collect();
        Self { version: CLASSIC_APPS_VERSION, apps }
    }
}

/// The cover files of the art folders, by app. A folder is read once, when the first cover is
/// saved into it, and kept up from then on: filling a library saves a cover per game, and reading
/// the folder for each of them reads every cover saved so far again.
#[derive(Default)]
pub struct ArtFiles {
    folders: HashMap<PathBuf, HashMap<i32, Vec<PathBuf>>>,
    #[cfg(test)]
    folders_read: usize,
}

impl ArtFiles {
    /// After a cover is saved at `keep` (`<app>.img` or `<app>-<version>.img`), removes the other
    /// files of the same app in that folder: older versions of the cover are never asked for again.
    pub fn saved(&mut self, keep: &Path, app_id: i32) {
        let Some(dir) = keep.parent() else { return };
        if !self.folders.contains_key(dir) {
            self.folders.insert(dir.to_owned(), art_files(dir));
            #[cfg(test)]
            {
                self.folders_read += 1;
            }
        }
        let Some(apps) = self.folders.get_mut(dir) else { return };
        let files = apps.entry(app_id).or_default();
        for old in files.iter().filter(|old| *old != keep) {
            let _ = std::fs::remove_file(old);
        }
        *files = vec![keep.to_owned()];
    }
}

/// The cover files in `dir`, by the app each is of.
fn art_files(dir: &Path) -> HashMap<i32, Vec<PathBuf>> {
    let mut apps: HashMap<i32, Vec<PathBuf>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return apps };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(app_id) = path.file_name().and_then(|n| n.to_str()).and_then(art_app) {
            apps.entry(app_id).or_default().push(path);
        }
    }
    apps
}

/// The app a cover file is of, by its name: `<app>.img` or `<app>-<version>.img` ([`Store::art_path`]).
fn art_app(name: &str) -> Option<i32> {
    let stem = name.strip_suffix(".img")?;
    stem.parse().ok().or_else(|| stem.rsplit_once('-')?.0.parse().ok())
}

fn sanitize(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect()
}

/// Splits "host", "host:port", "[v6]:port" or a bare IPv6 address.
pub fn parse_address(text: &str) -> Option<(String, Option<u16>)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p.parse().ok()?),
            None if tail.is_empty() => None,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    match text.matches(':').count() {
        0 => Some((text.to_owned(), None)),
        1 => {
            let (host, port) = text.split_once(':')?;
            if host.is_empty() {
                return None;
            }
            Some((host.to_owned(), Some(port.parse().ok()?)))
        }
        _ => Some((text.to_owned(), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_scales_with_pixels_and_rate() {
        assert_eq!(default_bitrate_kbps(1920, 1080, 60), 20_000);
        assert_eq!(default_bitrate_kbps(1280, 720, 60), 8_889);
        assert_eq!(default_bitrate_kbps(3840, 2160, 60), 80_000);
        assert!(default_bitrate_kbps(1920, 1080, 120) < 40_000);
        assert_eq!(default_bitrate_kbps(640, 360, 30), 2_000);
    }

    fn recent(host: &str, install: &str, app: u32, at: u64) -> Recent {
        Recent { host: host.into(), install_id: install.into(), app_id: app, title: format!("Game {app}"), at }
    }

    #[test]
    fn recents_put_the_latest_first_without_repeats_and_stay_capped() {
        let mut list = Vec::new();
        remember(&mut list, recent("pc", "A", 1, 10));
        remember(&mut list, recent("pc", "B", 2, 20));
        remember(&mut list, recent("other", "A", 1, 30));
        // The same game again (install ids compare without case) moves up instead of repeating.
        remember(&mut list, recent("pc", "a", 7, 40));
        assert_eq!(list.iter().map(|r| (r.host.as_str(), r.at)).collect::<Vec<_>>(), [("pc", 40), ("other", 30), ("pc", 20)]);
        // Without install ids the app id decides.
        remember(&mut list, recent("pc", "", 2, 50));
        remember(&mut list, recent("pc", "", 2, 60));
        assert_eq!(list.len(), 3);
        for n in 0..30 {
            remember(&mut list, recent("pc", &format!("G{n}"), 100 + n, 100 + u64::from(n)));
        }
        assert_eq!(list.len(), RECENT_LIMIT);
        assert_eq!(list[0].install_id, "G29");
    }

    #[test]
    fn the_automatic_start_defaults_to_the_pcs_and_keeps_its_state_over_a_restart() {
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.open_on_start, OpenOnStart::Pcs, "a settings file from before it starts nothing");
        let dir = std::env::temp_dir().join(format!("nightjar-open-play-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::at(dir.clone()).unwrap();
        assert_eq!(store.open_play(), OpenPlayState::default());
        let game = OpenGame { host: "uid".into(), install_id: "x".into(), app_id: 7, title: "Game".into() };
        store.save_open_play(&OpenPlayState { game: Some(game.clone()), armed: true }).unwrap();
        assert_eq!(Store::at(dir.clone()).unwrap().open_play(), OpenPlayState { game: Some(game), armed: true });
        store.forget_host("other", &[], &[]);
        assert!(store.open_play().game.is_some(), "another PC's removal leaves it");
        store.forget_host("uid", &[], &[]);
        assert_eq!(store.open_play(), OpenPlayState { game: None, armed: true }, "its PC's removal takes it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recents_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("nightjar-recent-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        assert!(store.recents().is_empty());
        let mut list = Vec::new();
        remember(&mut list, recent("pc", "A", 1, 10));
        store.save_recents(&list).unwrap();
        assert_eq!(Store::at(dir.clone()).unwrap().recents(), list);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_second_screens_store_reads_the_files_and_writes_none() {
        let dir = std::env::temp_dir().join(format!("nightjar-read-only-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::at(dir.clone()).unwrap();
        let settings = Settings { dual_screen: true, second_screen_width: 1280, second_screen_height: 720, ..Settings::default() };
        store.save_settings(&settings).unwrap();
        store.save_hosts(&[HostRecord { name: "pc".into(), ..HostRecord::default() }]).unwrap();
        let before: Vec<_> = ["settings.json", "hosts.json"].iter().map(|f| std::fs::read(dir.join(f)).unwrap()).collect();

        let second = Store::at(dir.clone()).unwrap().read_only();
        assert_eq!(second.settings(), settings, "it reads what the first wrote");
        assert_eq!(second.hosts()[0].name, "pc");
        second.save_settings(&Settings::default()).unwrap();
        second.save_hosts(&[]).unwrap();
        second.save_recents(&[recent("pc", "A", 1, 10)]).unwrap();
        second.save_hotkeys(&BTreeMap::from([("overlay".to_owned(), "Ctrl+O".to_owned())])).unwrap();
        let library = second.library_path("pc");
        std::fs::create_dir_all(library.parent().unwrap()).unwrap();
        std::fs::write(&library, "{}").unwrap();
        second.forget_host("pc", &[], &[]);
        assert!(library.exists(), "a PC's library stays");
        let after: Vec<_> = ["settings.json", "hosts.json"].iter().map(|f| std::fs::read(dir.join(f)).unwrap()).collect();
        assert_eq!(after, before);
        for written in ["recent.json", HOTKEYS_FILE] {
            assert!(!dir.join(written).exists(), "{written} written");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn one_unreadable_value_costs_neither_the_pcs_nor_the_other_settings() {
        let dir = std::env::temp_dir().join(format!("nightjar-unreadable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::at(dir.clone()).unwrap();
        let kept = |name: &str| std::fs::read_to_string(dir.join(name)).ok();

        // A codec a newer version wrote, as after a downgrade.
        let settings = r#"{"fps":90,"codec":"a newer version's codec"}"#;
        std::fs::write(dir.join("settings.json"), settings).unwrap();
        let loaded = store.settings();
        assert_eq!((loaded.fps, loaded.codec), (90, Settings::default().codec));
        assert_eq!(kept("settings.json.bad").as_deref(), Some(settings), "the next save replaces the file");

        // The same in one PC's and one game's settings, and a PC that can't be read at all.
        let hosts = r#"[
            {"unique_id":"a","name":"A","server_certificate":"pem","settings":{"fps":120,"codec":"newer"},
             "app_settings":{"id:7":{"codec":"newer","bitrate_kbps":5000}}},
            {"unique_id":"b","name":"B","http_port":"not a port"},
            {"unique_id":"c","name":"C","server_certificate":"pem","settings":null}]"#;
        std::fs::write(dir.join("hosts.json"), hosts).unwrap();
        let loaded = store.hosts();
        assert_eq!(loaded.iter().map(|host| host.unique_id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(loaded[0].server_certificate.as_deref(), Some("pem"), "the pairing stays");
        let pc = loaded[0].settings.as_ref().unwrap();
        assert!(pc.has(&["fps"]) && !pc.has(&["codec"]));
        assert_eq!(pc.apply(&Settings::default()).fps, 120);
        let game = &loaded[0].app_settings["id:7"];
        assert!(game.has(&["bitrate_kbps"]) && !game.has(&["codec"]));
        assert_eq!(loaded[1].settings, None);
        assert_eq!(kept("hosts.json.bad").as_deref(), Some(hosts));

        let profiles = r#"{"active":1,"entries":[{"id":1,"name":"Couch","settings":{"fps":30,"codec":"newer"}}]}"#;
        std::fs::write(dir.join("profiles.json"), profiles).unwrap();
        let loaded = store.profiles();
        assert_eq!(loaded.active, Some(1));
        assert!(loaded.entries[0].settings.has(&["fps"]) && !loaded.entries[0].settings.has(&["codec"]));
        assert_eq!(kept("profiles.json.bad").as_deref(), Some(profiles));

        // A file that is no JSON reads as none, and is kept beside the earlier copy.
        std::fs::write(dir.join("hosts.json"), "[{ cut off").unwrap();
        assert!(store.hosts().is_empty());
        assert_eq!(kept("hosts.json.bad.2").as_deref(), Some("[{ cut off"));

        // A file that reads whole leaves no copy, and the second screen's store writes none.
        std::fs::remove_file(dir.join("settings.json.bad")).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"fps":90}"#).unwrap();
        assert_eq!(store.settings().fps, 90);
        std::fs::write(dir.join("settings.json"), settings).unwrap();
        assert_eq!(Store::at(dir.clone()).unwrap().read_only().settings().fps, 90);
        assert_eq!(kept("settings.json.bad"), None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn connect_as_survives_a_restart_and_older_mappings_load_as_automatic() {
        let dir = std::env::temp_dir().join(format!("nightjar-connect-as-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        // A file from before the choice existed: the mapping stays and connects as Automatic.
        std::fs::write(dir.join("controllers.json"),
            r#"{"usb:1532:0717":{"name":"Razer Kishi","buttons":[null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null],"axes":[1,2,3,4,5,6],"deadzone":7}}"#).unwrap();
        let older = store.controller_mappings();
        assert_eq!(older["usb:1532:0717"].deadzone, 7);
        assert_eq!(older["usb:1532:0717"].connect_as, 0);
        let mut mapping = older["usb:1532:0717"].clone();
        mapping.connect_as = 1;
        let mappings = BTreeMap::from([("usb:1532:0717".to_string(), mapping)]);
        store.save_controller_mappings(&mappings).unwrap();
        assert_eq!(Store::at(dir.clone()).unwrap().controller_mappings(), mappings);
        // A type a newer Nightjar adds, or a value no build writes, reads as Automatic and keeps
        // this and every other mapping.
        for bad in ["4", "256", "-1", "\"x\"", "null", "1.5"] {
            std::fs::write(dir.join("controllers.json"), format!(
                r#"{{"usb:1532:0717":{{"name":"Razer Kishi","deadzone":7,"connect_as":{bad}}},"usb:045e:028e":{{"name":"Xbox 360","connect_as":2}}}}"#)).unwrap();
            let loaded = store.controller_mappings();
            assert_eq!(loaded.len(), 2, "{bad}");
            assert_eq!(loaded["usb:1532:0717"].deadzone, 7, "{bad}");
            assert_eq!(loaded["usb:1532:0717"].connect_as, 0, "{bad}");
            assert_eq!(loaded["usb:1532:0717"].arrival_type(2), 2, "{bad}");
            assert_eq!(loaded["usb:045e:028e"].connect_as, 2, "{bad}");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    fn star(host: &str, install: &str, app: u32) -> Favourite {
        Favourite { host: host.into(), install_id: install.into(), app_id: app, title: format!("Game {app}") }
    }

    #[test]
    fn a_star_goes_first_and_the_same_game_on_another_pc_is_its_own_favourite() {
        let mut list = Vec::new();
        assert!(toggle_favourite(&mut list, star("pc", "A", 1)));
        assert!(toggle_favourite(&mut list, star("other", "A", 1)));
        assert!(toggle_favourite(&mut list, star("pc", "B", 2)));
        assert_eq!(list.iter().map(|f| (f.host.as_str(), f.app_id)).collect::<Vec<_>>(), [("pc", 2), ("other", 1), ("pc", 1)]);
        // Starring again removes it (install ids compare without case), and only on that PC.
        assert!(!toggle_favourite(&mut list, star("pc", "a", 9)));
        assert_eq!(list.iter().map(|f| (f.host.as_str(), f.app_id)).collect::<Vec<_>>(), [("pc", 2), ("other", 1)]);
        // Without install ids the app id decides.
        assert!(toggle_favourite(&mut list, star("pc", "", 5)));
        assert!(!toggle_favourite(&mut list, star("pc", "", 5)));
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn hidden_games_survive_a_restart_and_show_again() {
        let dir = std::env::temp_dir().join(format!("nightjar-hidden-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        assert!(store.hidden_games().is_empty());
        let mut list = Vec::new();
        assert!(set_hidden(&mut list, vec![star("pc", "a", 1), star("pc2", "a", 1)], true));
        assert!(!set_hidden(&mut list, vec![star("pc", "a", 1)], true), "hidden already");
        assert_eq!(list.len(), 2);
        store.save_hidden_games(&list).unwrap();
        assert_eq!(Store::at(dir.clone()).unwrap().hidden_games(), list);
        assert!(set_hidden(&mut list, vec![star("pc", "A", 9)], false), "by install id, in any case");
        assert!(!set_hidden(&mut list, vec![star("pc", "other", 9)], false));
        assert!(set_hidden(&mut list, vec![star("pc", "a", 1)], true));
        assert!(set_hidden(&mut list, vec![star("pc", "uuid-of-the-pcs-list", 1)], false), "by app id, from the PC's list");
        assert_eq!(list, [star("pc2", "a", 1)]);
        std::fs::write(dir.join("hidden-games.json"), "{ not json").unwrap();
        assert!(store.hidden_games().is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("hidden-games.json.bak")).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn favourites_survive_a_restart_and_a_broken_file_reads_as_none() {
        let dir = std::env::temp_dir().join(format!("nightjar-favourites-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        assert!(store.favourites().is_empty());
        let mut list = Vec::new();
        toggle_favourite(&mut list, star("pc", "A", 1));
        toggle_favourite(&mut list, star("pc", "B", 2));
        store.save_favourites(&list).unwrap();
        assert_eq!(Store::at(dir.clone()).unwrap().favourites(), list);
        std::fs::write(dir.join("favourites.json"), "{ not json").unwrap();
        assert!(store.favourites().is_empty());
        // The unreadable file is kept aside, not silently lost.
        assert_eq!(std::fs::read_to_string(dir.join("favourites.json.bak")).unwrap(), "{ not json");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_bad_favourite_is_skipped_and_the_rest_and_the_original_are_kept() {
        let text = r#"[
            {"host":"pc","install_id":"A","app_id":1,"title":"One"},
            {"host":"pc"},
            {"host":"pc","install_id":"B","app_id":"x","title":"Two"},
            {"host":"pc","install_id":"C","app_id":3,"title":"Three"}
        ]"#;
        let (list, damaged) = parse_favourites(text);
        assert!(damaged);
        assert_eq!(list.iter().map(|f| f.title.as_str()).collect::<Vec<_>>(), ["One", "Three"]);
        assert_eq!(parse_favourites("[]"), (Vec::new(), false));

        let dir = std::env::temp_dir().join(format!("nightjar-favourites-bad-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        std::fs::write(dir.join("favourites.json"), text).unwrap();
        assert_eq!(store.favourites().len(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("favourites.json.bak")).unwrap(), text);
        std::fs::remove_dir_all(dir).ok();
    }

    /// stream_hotkeys.json is read entry by entry; one that doesn't read, or one a newer Nightjar
    /// wrote, is never lost to the next change.
    #[test]
    fn a_corrupt_or_newer_hotkeys_file_is_kept() {
        let dir = std::env::temp_dir().join(format!("nightjar-hotkeys-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let store = Store::at(dir.clone()).unwrap();
        let path = dir.join("stream_hotkeys.json");
        assert!(store.hotkeys().is_empty(), "no file");
        let bindings = nightjar_policy::shortcuts::HotkeyBindings::from([("quit".to_string(), "CTRL+F9".to_string())]);
        store.save_hotkeys(&bindings).unwrap();
        assert_eq!(store.hotkeys(), bindings);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, serde_json::json!({"version": 1, "bindings": {"quit": "CTRL+F9"}}));

        // One bad value: the others still read, and it stays in the file.
        let text = r#"{"version": 1, "bindings": {"quit": "CTRL+F9", "overlay": 7, "capture": "off"}}"#;
        std::fs::write(&path, text).unwrap();
        let read = store.hotkeys();
        assert_eq!((read.get("quit").map(String::as_str), read.get("capture").map(String::as_str), read.get("overlay")), (Some("CTRL+F9"), Some("off"), None));
        store.save_hotkeys(&read).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["bindings"]["overlay"], 7);

        // A newer version: the entries this one knows are read, the rest and the version kept, the file copied first.
        let newer = r#"{"version": 2, "bindings": {"quit": "CTRL+F9", "teleport": "CTRL+T", "overlay": {"keys": ["CTRL", "O"]}}, "extra": true}"#;
        std::fs::write(&path, newer).unwrap();
        let read = store.hotkeys();
        assert_eq!(read.get("quit").map(String::as_str), Some("CTRL+F9"));
        let mut changed = read.clone();
        changed.insert("capture".into(), "off".into());
        store.save_hotkeys(&changed).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("stream_hotkeys.json.v2")).unwrap(), newer);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["bindings"]["teleport"], "CTRL+T");
        assert_eq!(saved["bindings"]["overlay"], serde_json::json!({"keys": ["CTRL", "O"]}));
        assert_eq!(saved["bindings"]["capture"], "off");

        // Not the format at all: defaults, and the file is copied before a change replaces it.
        for (n, broken) in ["{not json", r#"{"quit": "CTRL+F9"}"#].into_iter().enumerate() {
            std::fs::write(&path, broken).unwrap();
            assert!(store.hotkeys().is_empty());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), broken, "reading never rewrites it");
            store.save_hotkeys(&bindings).unwrap();
            let copy = if n == 0 { "stream_hotkeys.json.unreadable".to_string() } else { format!("stream_hotkeys.json.unreadable.{}", n + 1) };
            assert_eq!(std::fs::read_to_string(dir.join(copy)).unwrap(), broken);
            assert_eq!(store.hotkeys(), bindings);
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn removing_a_pc_clears_its_covers_recent_games_and_favourites() {
        let dir = std::env::temp_dir().join(format!("nightjar-forget-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        let mut list = Vec::new();
        remember(&mut list, recent("gone", "A", 1, 10));
        remember(&mut list, recent("kept", "B", 2, 20));
        store.save_recents(&list).unwrap();
        for host in ["gone", "kept"] {
            let cover = store.art_path(host, 1, "");
            std::fs::create_dir_all(cover.parent().unwrap()).unwrap();
            std::fs::write(&cover, b"x").unwrap();
        }
        let mut stars = Vec::new();
        toggle_favourite(&mut stars, star("gone", "A", 1));
        toggle_favourite(&mut stars, star("kept", "B", 2));
        store.save_favourites(&stars).unwrap();
        let (after, starred) = store.forget_host("gone", &list, &stars);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].host, "kept");
        assert_eq!(store.recents(), after, "the file is filtered too");
        assert_eq!(starred.iter().map(|f| f.host.as_str()).collect::<Vec<_>>(), ["kept"]);
        assert_eq!(store.favourites(), starred, "favourites.json is filtered too");
        assert!(!store.art_path("gone", 1, "").exists(), "its covers are deleted");
        assert!(store.art_path("kept", 1, "").exists(), "another PC's are not");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn addresses_parse() {
        assert_eq!(parse_address("pc.local"), Some(("pc.local".into(), None)));
        assert_eq!(
            parse_address(" 10.0.0.2:48989 "),
            Some(("10.0.0.2".into(), Some(48989)))
        );
        assert_eq!(parse_address("[fe80::1]:47989"), Some(("fe80::1".into(), Some(47989))));
        assert_eq!(parse_address("fe80::1"), Some(("fe80::1".into(), None)));
        assert_eq!(parse_address("pc:notaport"), None);
        assert_eq!(parse_address(""), None);
    }

    #[test]
    fn saved_libraries_and_versioned_covers() {
        let library: nightjar_core::http::native_api::library::Library = serde_json::from_str(include_str!("../../client/tests/fixtures/native-api/library.json")).unwrap();
        let dir = std::env::temp_dir().join(format!("nightjar-library-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        let host = "5E0F4B1C-7A3D-4C52-9E10-0B6F2D8A4C11";
        assert!(store.library(host).is_none());
        let mut library = NativeLibrary::from(library);
        library.last_played.insert(library.library.games[0].install_id.clone(), 1788025796);
        store.save_library(host, &library).unwrap();
        let back = store.library(host).unwrap();
        assert_eq!((back.library.revision.as_str(), back.library.games.len()), ("9f2c41d0", 2));
        assert_eq!(back, library, "with when its games were last played");
        // Another PC does not see it, and a path-like id cannot leave the folder.
        assert!(store.library("other").is_none());
        assert!(store.library_path("../../x").starts_with(dir.join("library")));
        store.forget_library(host);
        assert!(store.library(host).is_none());
        // A new cover version is a new file; no version keeps the classic name.
        assert_ne!(store.art_path(host, 7, "a41c09e2"), store.art_path(host, 7, "0c1d2e3f"));
        assert!(store.art_path(host, 7, "").ends_with("7.img"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_classic_app_list_is_saved_without_its_control_tiles_and_goes_with_the_library() {
        use nightjar_core::http::{App, AppControl};
        let app = |id: i32, name: &str, uuid: &str, control: AppControl| App {
            name: name.into(),
            id,
            hdr_supported: false,
            uuid: uuid.into(),
            host_index: None,
            art_version: if id == 1 { "v2".into() } else { String::new() },
            control,
            monitor_resume: false,
        };
        let apps = ClassicApps::of(&[
            app(1, "Desktop", "A1", AppControl::None),
            app(2, "Terminate", "E16CBE1B-295D-4632-9A76-EC4180C857D3", AppControl::Terminate),
            app(3, "Hades", "", AppControl::None),
        ]);
        assert_eq!(apps.version, CLASSIC_APPS_VERSION);
        assert_eq!(apps.apps.iter().map(|a| (a.id, a.name.as_str(), a.uuid.as_str(), a.art_version.as_str())).collect::<Vec<_>>(), [(1, "Desktop", "A1", "v2"), (3, "Hades", "", "")]);

        let dir = std::env::temp_dir().join(format!("nightjar-classic-apps-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        let host = "pc-1";
        assert!(store.classic_apps(host).is_none());
        store.save_classic_apps(host, &apps).unwrap();
        assert_eq!(store.classic_apps(host), Some(apps.clone()));
        assert!(store.classic_apps("other").is_none());
        assert!(store.classic_apps_path("../../x").starts_with(dir.join("apps")));
        // An older or newer file reads too: missing fields take defaults, unknown ones are ignored.
        std::fs::write(store.classic_apps_path("old"), r#"{"apps":[{"id":5}]}"#).unwrap();
        let old = store.classic_apps("old").unwrap();
        assert_eq!((old.version, old.apps[0].id, old.apps[0].name.as_str()), (0, 5, ""));
        std::fs::write(store.classic_apps_path("new"), r#"{"version":2,"apps":[{"id":5,"name":"A","hdr":true}],"more":1}"#).unwrap();
        assert_eq!(store.classic_apps("new").unwrap().apps[0].name, "A");
        // Unpairing or removing the PC drops it with the native library.
        store.forget_library(host);
        assert!(store.classic_apps(host).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_a_cover_prunes_the_older_versions_of_that_app_only() {
        let dir = std::env::temp_dir().join(format!("nightjar-art-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        let host = "5E0F4B1C";
        let old = store.art_path(host, 7, "a41c09e2");
        let new = store.art_path(host, 7, "0c1d2e3f");
        let plain = store.art_path(host, 7, "");
        let other_app = store.art_path(host, 70, "a41c09e2");
        let other_app_plain = store.art_path(host, 8, "");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        for path in [&old, &new, &plain, &other_app, &other_app_plain] {
            std::fs::write(path, b"x").unwrap();
        }
        let mut files = ArtFiles::default();
        files.saved(&new, 7);
        assert!(new.exists(), "the cover just saved stays");
        assert!(!old.exists() && !plain.exists(), "older versions of app 7 go");
        assert!(other_app.exists() && other_app_plain.exists(), "other apps keep theirs (70 is not 7)");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A library's covers are saved one after another: the folder is read for the first of them
    /// only, and every one still leaves its app a single file.
    #[test]
    fn saving_many_covers_reads_the_folder_once_and_leaves_no_older_version() {
        let dir = std::env::temp_dir().join(format!("nightjar-art-many-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        let host = "5E0F4B1C";
        let folder = store.art_path(host, 0, "").parent().unwrap().to_owned();
        std::fs::create_dir_all(&folder).unwrap();
        // What an earlier run left: a versioned and a plain cover of each app.
        for app in 1..=20 {
            std::fs::write(store.art_path(host, app, "old"), b"x").unwrap();
            std::fs::write(store.art_path(host, app, ""), b"x").unwrap();
        }
        let mut files = ArtFiles::default();
        for app in 1..=20 {
            let new = store.art_path(host, app, "new");
            std::fs::write(&new, b"x").unwrap();
            files.saved(&new, app);
        }
        // A cover that changes again in the same run is known without the folder.
        let newer = store.art_path(host, 7, "newer");
        std::fs::write(&newer, b"x").unwrap();
        files.saved(&newer, 7);
        assert_eq!(files.folders_read, 1, "not once per cover");
        let mut left: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        let mut expected: Vec<String> = (1..=20).map(|app| if app == 7 { "7-newer.img".to_owned() } else { format!("{app}-new.img") }).collect();
        expected.sort();
        assert_eq!(left, expected, "one file per app, the newest");

        // Another PC's folder is its own.
        let other = store.art_path("0A1B2C3D", 7, "new");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(store.art_path("0A1B2C3D", 7, ""), b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        files.saved(&other, 7);
        assert_eq!(files.folders_read, 2);
        assert!(other.exists() && !store.art_path("0A1B2C3D", 7, "").exists() && newer.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_cover_file_names_its_app() {
        assert_eq!(art_app("7.img"), Some(7));
        assert_eq!(art_app("7-0c1d2e3f.img"), Some(7));
        assert_eq!(art_app("881448767-881448767.img"), Some(881448767), "a version of digits is no app id");
        assert_eq!(art_app("-5.img"), Some(-5));
        assert_eq!(art_app("-5-a41c09e2.img"), Some(-5));
        assert_eq!(art_app("7.png"), None);
        assert_eq!(art_app("notes.img"), None);
    }

    #[test]
    fn home_is_on_unless_the_file_says_otherwise() {
        assert!(Settings::default().console_home);
        let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
        assert!(old.console_home, "a settings file from before Home keeps it on");
        let off: Settings = serde_json::from_str("{\"console_home\": false}").unwrap();
        assert!(!off.console_home);
    }

    #[test]
    fn a_settings_file_from_before_accessibility_loads_with_the_defaults() {
        let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
        assert_eq!(old.ui_zoom, 100);
        assert!(!old.high_contrast);
        assert_eq!(old.reduce_motion, ReduceMotion::System);
        assert_eq!(old.language, Language::System);
        let set: Settings =
            serde_json::from_str("{\"ui_zoom\":150,\"high_contrast\":true,\"reduce_motion\":\"Reduce\",\"language\":\"German\"}").unwrap();
        assert_eq!((set.ui_zoom, set.high_contrast, set.reduce_motion, set.language), (150, true, ReduceMotion::Reduce, Language::German));
    }

    #[test]
    fn a_value_from_a_newer_version_does_not_reset_the_other_settings() {
        let set: Settings = serde_json::from_str("{\"fps\":90,\"reduce_motion\":\"Sometimes\",\"language\":\"Klingon\",\"ui_zoom\":130}").unwrap();
        assert_eq!((set.fps, set.ui_zoom), (90, 130));
        assert_eq!((set.reduce_motion, set.language), (ReduceMotion::System, Language::System));
    }

    #[test]
    fn zoom_steps_stay_inside_the_range_and_odd_values_snap() {
        assert_eq!(step_zoom(100, 1), 115);
        assert_eq!(step_zoom(100, -1), 85);
        assert_eq!(step_zoom(150, 1), 150, "the top step stays");
        assert_eq!(step_zoom(85, -1), 85, "the bottom step stays");
        assert_eq!(nearest_zoom(0), 85);
        assert_eq!(nearest_zoom(9999), 150);
        assert_eq!(nearest_zoom(120), 115);
        assert_eq!(step_zoom(120, 1), 130);
    }

    #[test]
    fn early_decode_is_off_unless_the_file_says_otherwise() {
        assert!(!Settings::default().early_decode);
        let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
        assert!(!old.early_decode, "a settings file from before early decode keeps it off");
        let on: Settings = serde_json::from_str("{\"early_decode\": true}").unwrap();
        assert!(on.early_decode);
    }

    #[test]
    fn pyrowave_async_compute_is_off_unless_the_file_says_otherwise() {
        assert!(!Settings::default().pyrowave_async_compute);
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert!(!old.pyrowave_async_compute, "a settings file from before the setting keeps it off");
        let on: Settings = serde_json::from_str("{\"pyrowave_async_compute\": true}").unwrap();
        assert!(on.pyrowave_async_compute);
        let saved = serde_json::to_value(&on).unwrap();
        assert_eq!(saved["pyrowave_async_compute"], serde_json::Value::Bool(true), "saved under its key");
        let back: Settings = serde_json::from_value(saved).unwrap();
        assert!(back.pyrowave_async_compute);
    }

    #[test]
    fn beam_race_is_off_unless_the_file_says_otherwise() {
        assert!(!Settings::default().beam_race);
        let old: Settings = serde_json::from_str("{\"early_decode\": true}").unwrap();
        assert!(!old.beam_race, "a settings file from before the beam race keeps it off");
        let on: Settings = serde_json::from_str("{\"beam_race\": true}").unwrap();
        assert!(on.beam_race);
        assert_eq!(old.beam_race_margin_ms, 0, "the margin is automatic unless the file sets one");
        let margin: Settings = serde_json::from_str("{\"beam_race_margin_ms\": 2}").unwrap();
        assert_eq!(margin.beam_race_margin_ms, 2);
    }

    #[test]
    fn pyrowave_decoder_uses_the_platform_default_and_reads_from_the_file() {
        let default = if cfg!(target_os = "macos") { PyroWaveDecoder::Cpu } else { PyroWaveDecoder::Gpu };
        assert_eq!(Settings::default().pyrowave_decoder, default);
        let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
        assert_eq!(old.pyrowave_decoder, default, "an older file gets the default");
        let cpu: Settings = serde_json::from_str("{\"pyrowave_decoder\": \"Cpu\"}").unwrap();
        assert_eq!(cpu.pyrowave_decoder, PyroWaveDecoder::Cpu, "a saved CPU choice stays");
        let gpu: Settings = serde_json::from_str("{\"pyrowave_decoder\": \"Gpu\"}").unwrap();
        assert_eq!(gpu.pyrowave_decoder, PyroWaveDecoder::Gpu);
        let mut overrides = Overrides::default();
        overrides.0.insert("pyrowave_decoder".into(), serde_json::json!("Gpu"));
        assert_eq!(overrides.apply(&Settings::default()).pyrowave_decoder, PyroWaveDecoder::Gpu);
    }

    #[test]
    fn yuv444_and_sdr10_default_off_and_ask_for_the_gpu_renderer() {
        let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
        assert!(!old.prefer_yuv444 && !old.sdr_10bit, "an older file gets them off");
        assert!(!Settings::default().needs_gpu());
        let on: Settings = serde_json::from_str("{\"prefer_yuv444\": true}").unwrap();
        assert!(on.needs_gpu(), "Linux draws 4:4:4 pictures with the plane pass");
        let on: Settings = serde_json::from_str("{\"sdr_10bit\": true}").unwrap();
        assert!(on.needs_gpu(), "and software 10-bit pictures too");
        // A game or PC that turns one on starts Nightjar with the GPU renderer as well.
        let mut overrides = Overrides::default();
        assert!(!overrides.needs_gpu());
        overrides.0.insert("sdr_10bit".into(), serde_json::json!(true));
        assert!(overrides.needs_gpu());
    }

    #[test]
    fn hdr_brightness_is_the_pcs_by_default_and_a_pc_can_follow_the_desktop() {
        assert!(!Settings::default().hdr_desktop_brightness, "absolute: what HDR10 means and direct scanout needs");
        let old: Settings = serde_json::from_str("{\"hdr\": true}").unwrap();
        assert!(!old.hdr_desktop_brightness, "an older file gets the default");
        let on: Settings = serde_json::from_str("{\"hdr_desktop_brightness\": true}").unwrap();
        assert!(on.hdr_desktop_brightness);
        let saved = serde_json::to_value(&on).unwrap();
        assert_eq!(saved["hdr_desktop_brightness"], serde_json::json!(true), "saved under its own key");
        let mut overrides = Overrides::default();
        overrides.0.insert("hdr_desktop_brightness".into(), serde_json::json!(true));
        assert!(overrides.apply(&Settings::default()).hdr_desktop_brightness, "a PC's own setting");
        assert!(!overrides.needs_gpu(), "the renderer HDR takes is enough");
    }

    #[test]
    fn frame_warp_is_off_by_default_and_saved_under_its_own_key() {
        assert!(!Settings::default().frame_warp, "off until the player turns it on");
        let old: Settings = serde_json::from_str("{\"upscale_sharpness\": 70}").unwrap();
        assert!(!old.frame_warp, "an older file gets the default");
        let on: Settings = serde_json::from_str("{\"frame_warp\": true}").unwrap();
        assert!(on.frame_warp);
        let saved = serde_json::to_value(&on).unwrap();
        assert_eq!(saved["frame_warp"], serde_json::json!(true), "saved under its own key");
        let again: Settings = serde_json::from_value(saved).unwrap();
        assert_eq!(again, on, "it round-trips");
        let mut overrides = Overrides::default();
        overrides.0.insert("frame_warp".into(), serde_json::json!(true));
        assert!(overrides.apply(&Settings::default()).frame_warp, "a PC's or game's own setting");
        assert!(!overrides.needs_gpu(), "every renderer draws it");
    }

    #[test]
    fn the_renderer_choice_defaults_to_auto_and_reads_from_the_file() {
        assert_eq!(Settings::default().renderer, Renderer::Auto);
        let old: Settings = serde_json::from_str("{\"fps\": 90}").unwrap();
        assert_eq!(old.renderer, Renderer::Auto, "an older file gets the default");
        for choice in Renderer::ALL {
            let saved = serde_json::to_string(&Settings { renderer: choice, ..Settings::default() }).unwrap();
            assert_eq!(serde_json::from_str::<Settings>(&saved).unwrap().renderer, choice);
        }
        let gpu: Settings = serde_json::from_str("{\"renderer\": \"Gpu\"}").unwrap();
        assert_eq!(gpu.renderer, Renderer::Gpu);
        let opengl: Settings = serde_json::from_str("{\"renderer\": \"OpenGl\"}").unwrap();
        assert_eq!(opengl.renderer, Renderer::OpenGl);
    }

    #[test]
    fn the_gpu_need_names_hdr_only_when_nothing_else_needs_the_renderer() {
        assert_eq!(Settings::default().gpu_need(), None);
        let mut settings = Settings { hdr: true, ..Settings::default() };
        assert_eq!(settings.gpu_need(), Some("HDR"));
        settings.upscaling = Upscaling::Fsr1;
        assert_eq!(settings.gpu_need(), Some("upscaling"), "a stream that needs the renderer even in SDR");
        settings.codec = CodecPreference::PyroWave;
        assert_eq!(settings.gpu_need(), Some("PyroWave"));
    }

    #[test]
    fn files_round_trip() {
        let dir = std::env::temp_dir().join(format!("nightjar-config-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        assert_eq!(store.settings(), Settings::default());
        let settings = Settings {
            fps: 120,
            codec: CodecPreference::Av1,
            ..Default::default()
        };
        store.save_settings(&settings).unwrap();
        assert_eq!(store.settings(), settings);
        let mut profiles = Profiles::default();
        let id = profiles.create("Travel", serde_json::from_str(r#"{"fps":60,"hdr":false}"#).unwrap()).unwrap();
        profiles.active = Some(id);
        store.save_profiles(&profiles).unwrap();
        assert_eq!(store.profiles(), profiles);
        let mut shortcuts = crate::shortcuts::Shortcuts::default();
        crate::shortcuts::save(&mut shortcuts, None, "Task manager", "CTRL + SHIFT + ESC").unwrap();
        store.save_shortcuts(&shortcuts).unwrap();
        assert_eq!(store.shortcuts(), shortcuts);
        // Stream hotkeys hold across a restart.
        assert!(store.hotkeys().is_empty());
        let hotkeys = BTreeMap::from([("quit".to_string(), "CTRL+F9".to_string()), ("overlay".into(), "off".into())]);
        store.save_hotkeys(&hotkeys).unwrap();
        assert_eq!(store.hotkeys(), hotkeys);
        let mapping = crate::gamepad::Mapping { name: "Test pad".into(), deadzone: 12, ..Default::default() };
        let mut mappings = BTreeMap::from([("usb:1234:0001".into(), mapping)]);
        store.save_controller_mappings(&mappings).unwrap();
        assert_eq!(store.controller_mappings(), mappings);
        mappings.get_mut("usb:1234:0001").unwrap().axes[0] = -128;
        store.save_controller_mappings(&mappings).unwrap();
        assert!(store.controller_mappings().is_empty());
        let mut hosts = vec![HostRecord {
            unique_id: "ABC".into(),
            name: "Desk".into(),
            address: "10.0.0.2".into(),
            http_port: 47989,
            server_certificate: Some("PEM".into()),
            ..Default::default()
        }];
        hosts[0].save_app_settings("game-a", 1, serde_json::from_str(r#"{"mouse_mode":"Absolute"}"#).unwrap());
        store.save_hosts(&hosts).unwrap();
        assert_eq!(store.hosts(), hosts);
        // A settings file from an older version keeps the new fields' defaults.
        std::fs::write(dir.join("settings.json"), r#"{"fps": 90}"#).unwrap();
        assert_eq!(store.settings().fps, 90);
        assert!(!store.settings().present_at_host_times);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn host_settings_inherit_until_overridden() {
        let mut host: HostRecord = serde_json::from_str(r#"{"address":"pc.local"}"#).unwrap();
        let mut global = Settings { fps: 120, ..Default::default() };
        assert_eq!(host.settings(&global).fps, 120);
        host.settings = Some(serde_json::from_str(r#"{"fps":60,"microphone":true}"#).unwrap());
        global.fps = 144;
        let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&host).unwrap()).unwrap();
        assert_eq!(saved.settings(&global).fps, 60);
        assert!(saved.settings(&global).microphone);
        host.settings = None;
        assert_eq!(host.settings(&global).fps, 144);
    }

    #[test]
    fn a_pcs_own_settings_count_each_game_once_and_clear_to_the_layers_below() {
        // As 4.1.0 wrote it: Frame rate pinned for the PC, one game with a UUID (saved under both
        // keys) and one without.
        let mut host: HostRecord = serde_json::from_value(serde_json::json!({
            "address": "pc.local",
            "settings": { "fps": 120 },
            "app_settings": { "uuid:a": { "hdr": true }, "id:1": { "hdr": true }, "id:2": { "fps": 30 } },
        })).unwrap();
        assert_eq!(host.own_settings(), (true, 2));
        host.clear_own_settings();
        assert_eq!(host.own_settings(), (false, 0));
        let main = Settings { fps: 60, ..Default::default() };
        assert_eq!(host.settings(&main).fps, 60);
        assert!(host.app_settings("a", 1).is_none());

        let untouched: HostRecord = serde_json::from_str(r#"{"address":"pc.local","settings":{}}"#).unwrap();
        assert_eq!(untouched.own_settings(), (false, 0));
        // Settings saved before the look at kept settings have it still to run; after, it stays done.
        let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
        assert!(!old.kept_settings_checked);
        let done = Settings { kept_settings_checked: true, ..old };
        assert!(serde_json::from_str::<Settings>(&serde_json::to_string(&done).unwrap()).unwrap().kept_settings_checked);
    }

    #[test]
    fn a_setting_names_the_layer_it_comes_from() {
        let profile = Profile { id: 1, name: "Travel".into(), settings: serde_json::from_str(r#"{"bitrate_kbps":5000}"#).unwrap() };
        let pc: Overrides = serde_json::from_str(r#"{"fps":120,"bitrate_kbps":9000}"#).unwrap();
        let game: Overrides = serde_json::from_str(r#"{"hdr":true}"#).unwrap();
        assert_eq!(source(&["fps"], Some(&profile), Some(&pc), Some(&game)), Source::Pc);
        assert_eq!(source(&["bitrate_kbps"], Some(&profile), None, Some(&game)), Source::Profile("Travel".into()));
        assert_eq!(source(&["hdr"], Some(&profile), Some(&pc), Some(&game)), Source::Game);
        assert_eq!(source(&["width", "height"], Some(&profile), Some(&pc), Some(&game)), Source::Main);
        assert_eq!(source(&["fps"], None, None, None), Source::Main);
    }

    #[test]
    fn a_pc_alias_round_trips_and_an_old_file_has_none() {
        let old: HostRecord = serde_json::from_str(r#"{"address":"pc.local","name":"DESKTOP-1"}"#).unwrap();
        assert_eq!(old.alias, None);
        let mut host = old;
        host.set_alias("  Living room  ");
        assert_eq!(host.alias.as_deref(), Some("Living room"));
        let saved: HostRecord = serde_json::from_str(&serde_json::to_string(&host).unwrap()).unwrap();
        assert_eq!(saved.alias.as_deref(), Some("Living room"));
        assert_eq!(saved.name, "DESKTOP-1", "the PC's own name is untouched");
        host.set_alias(&"x".repeat(60));
        assert_eq!(host.alias.as_ref().map(|a| a.chars().count()), Some(ALIAS_MAX_CHARS));
        host.set_alias("   ");
        assert_eq!(host.alias, None, "an empty name restores the PC's own");
    }

    #[test]
    fn partial_profiles_and_game_overrides_follow_their_layers() {
        let mut global = Settings { fps: 120, microphone: true, ..Default::default() };
        let mut profiles = Profiles::default();
        let id = profiles.create(" Home ", serde_json::from_str(r#"{"codec":"Av1","fps":90}"#).unwrap()).unwrap();
        profiles.active = Some(id);
        assert!(profiles.create("HOME", Overrides::default()).is_err());
        let mut host = HostRecord { settings: Some(serde_json::from_str(r#"{"fps":60}"#).unwrap()), ..Default::default() };
        host.save_app_settings("game-a", 1, serde_json::from_str(r#"{"hdr":true,"microphone":false}"#).unwrap());
        global.upscale_sharpness = 80;
        let base = host.settings(&profiles.settings(&global, crate::network::Kind::Other));
        let game = host.app_settings("game-a", 1).unwrap().apply(&base);
        assert_eq!(game.fps, 60);
        assert_eq!(game.codec, CodecPreference::Av1);
        assert_eq!(game.upscale_sharpness, 80);
        assert!(game.hdr);
        assert!(!game.microphone);
        assert!(host.app_settings("game-b", 1).is_none());
        assert!(host.app_settings("game-a", 7).is_some());
        assert!(host.app_settings("", 1).is_some());
        host.save_app_settings("game-a", 1, Overrides::default());
        assert!(host.app_settings.is_empty());
        profiles.delete(id);
        assert_eq!(profiles.active, None);
        assert_eq!(profiles.settings(&global, crate::network::Kind::Other), global);
    }

    #[test]
    fn network_choices_fall_back_and_deleted_rules_do_not_select_reused_ids() {
        use crate::network::{Kind, Network};
        let mut profiles = Profiles::default();
        let active = profiles.create("Default", serde_json::from_str(r#"{"fps":60}"#).unwrap()).unwrap();
        let travel = profiles.create("Travel", serde_json::from_str(r#"{"fps":30}"#).unwrap()).unwrap();
        profiles.active = Some(active);
        profiles.mobile = Some(travel);
        profiles.wifi = Some(travel);
        profiles.home = Some(active);
        profiles.home_wifi.push("Home".into());
        profiles.mobile_limit_kbps = 5000;
        let mut network = Network { wifi: true, ssid: Some("Home".into()), ..Default::default() };
        assert_eq!(network.kind(&profiles.home_wifi), Kind::HomeWifi);
        assert_eq!(profiles.selected(Kind::Mobile), Some(active));
        profiles.automatic = true;
        assert_eq!(profiles.settings(&Settings::default(), Kind::Mobile).fps, 30);
        assert_eq!(profiles.selected(Kind::Other), Some(active));
        network.metered = true;
        assert_eq!(network.kind(&profiles.home_wifi), Kind::Mobile);
        assert_eq!(profiles.bitrate_limit(Kind::Mobile), 5000);
        assert_eq!(profiles.bitrate_limit(Kind::HomeWifi), 0);
        network.metered = false;
        network.ssid = None;
        assert_eq!(network.kind(&profiles.home_wifi), Kind::Wifi);
        profiles.delete(travel);
        assert_eq!((profiles.mobile, profiles.wifi), (None, None));
        assert_eq!(profiles.selected(Kind::Mobile), Some(active));
        profiles.mobile = Some(999);
        assert_eq!(profiles.settings(&Settings::default(), Kind::Mobile), Settings::default());
        let restored: Profiles = serde_json::from_str(&serde_json::to_string(&profiles).unwrap()).unwrap();
        assert_eq!(restored, profiles);
    }

    #[test]
    fn telling_the_host_about_wireless_is_on_by_default_and_in_old_files() {
        assert!(Settings::default().tell_host_wireless);
        // A settings file from before the field existed keeps it on.
        let old: Settings = serde_json::from_str(r#"{"fps":60,"auto_reconnect":true}"#).unwrap();
        assert!(old.tell_host_wireless && old.auto_reconnect);
        let off: Settings = serde_json::from_str(r#"{"tell_host_wireless":false}"#).unwrap();
        assert!(!off.tell_host_wireless);
    }

    #[test]
    fn client_roaming_is_on_and_mobile_data_off_by_default_and_in_old_files() {
        let defaults = Settings::default();
        assert!(defaults.client_roaming && !defaults.roaming_metered);
        let old: Settings = serde_json::from_str(r#"{"fps":60,"tell_host_wireless":false}"#).unwrap();
        assert!(old.client_roaming && !old.roaming_metered);
        let set: Settings = serde_json::from_str(r#"{"client_roaming":false,"roaming_metered":true}"#).unwrap();
        assert!(!set.client_roaming && set.roaming_metered);
    }

    #[test]
    fn grouped_video_packets_are_on_by_default_and_in_old_files() {
        assert!(Settings::default().receive_coalescing);
        let old: Settings = serde_json::from_str(r#"{"fps":60,"client_roaming":false}"#).unwrap();
        assert!(old.receive_coalescing);
        let off: Settings = serde_json::from_str(r#"{"receive_coalescing":false}"#).unwrap();
        assert!(!off.receive_coalescing);
    }

    #[test]
    fn recover_interrupted_streams_turns_on_once_and_a_later_off_is_kept() {
        assert!(Settings::default().auto_reconnect);
        let dir = std::env::temp_dir().join(format!("nightjar-reconnect-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"auto_reconnect": false}"#).unwrap();
        let mut settings = store.settings();
        assert!(settings.auto_reconnect, "an old file's off is switched on once");
        settings.auto_reconnect = false;
        store.save_settings(&settings).unwrap();
        assert!(!store.settings().auto_reconnect, "the user's off after the switch is kept");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_saved_pc_render_resolution_is_kept_in_androids_range() {
        let dir = std::env::temp_dir().join(format!("nightjar-render-scale-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        for (saved, read) in [(300, 200), (225, 200), (200, 200), (150, 150), (50, 50), (25, 50)] {
            std::fs::write(dir.join("settings.json"), format!(r#"{{"virtual_display_scale": {saved}}}"#)).unwrap();
            assert_eq!(store.settings().virtual_display_scale, read, "settings.json at {saved} %");
            // A profile's, PC's or game's own value is kept in the range too.
            let own: Overrides = serde_json::from_str(&format!(r#"{{"virtual_display_scale": {saved}}}"#)).unwrap();
            assert_eq!(own.apply(&Settings::default()).virtual_display_scale, read, "an override at {saved} %");
        }
        assert_eq!(Settings::default().virtual_display_scale, 100);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_saved_pc_display_scaling_windows_does_not_offer_keeps_the_pcs_own() {
        let dir = std::env::temp_dir().join(format!("nightjar-display-scale-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        for (saved, read) in [(175, 175), (500, 500), (0, 0), (275, 0), (600, 0), (99, 0)] {
            std::fs::write(dir.join("settings.json"), format!(r#"{{"virtual_display_dpi": {saved}}}"#)).unwrap();
            assert_eq!(store.settings().virtual_display_dpi, read, "settings.json at {saved} %");
            let own: Overrides = serde_json::from_str(&format!(r#"{{"virtual_display_dpi": {saved}}}"#)).unwrap();
            assert_eq!(own.apply(&Settings::default()).virtual_display_dpi, read, "an override at {saved} %");
        }
        // Files from before the setting keep the PC's own scale and the render resolution apart.
        std::fs::write(dir.join("settings.json"), r#"{"virtual_display_scale": 150}"#).unwrap();
        assert_eq!((store.settings().virtual_display_dpi, store.settings().virtual_display_scale), (0, 150));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clipboard_sync_is_off_by_default_and_in_old_files() {
        assert!(!Settings::default().clipboard_sync);
        assert!(!serde_json::from_str::<Settings>("{}").unwrap().clipboard_sync);
        assert!(serde_json::from_str::<Settings>(r#"{"clipboard_sync":true}"#).unwrap().clipboard_sync);
    }

    #[test]
    fn usb_forwarding_is_off_by_default_and_in_old_files() {
        assert!(!Settings::default().usb_forwarding);
        assert!(!serde_json::from_str::<Settings>("{}").unwrap().usb_forwarding);
        assert!(serde_json::from_str::<Settings>(r#"{"usb_forwarding":true}"#).unwrap().usb_forwarding);
        assert!(Settings::default().usb_auto.is_empty());
        assert!(serde_json::from_str::<Settings>("{}").unwrap().usb_auto.is_empty());
        assert_eq!(serde_json::from_str::<Settings>(r#"{"usb_auto":["28de:1304"]}"#).unwrap().usb_auto, ["28de:1304"]);
    }

    #[test]
    fn telling_the_host_the_link_speed_is_on_by_default_and_in_old_files() {
        assert!(Settings::default().tell_host_link_speed);
        let old: Settings = serde_json::from_str(r#"{"fps":60,"tell_host_wireless":false}"#).unwrap();
        assert!(old.tell_host_link_speed && !old.tell_host_wireless);
        let off: Settings = serde_json::from_str(r#"{"tell_host_link_speed":false}"#).unwrap();
        assert!(!off.tell_host_link_speed);
    }

    #[test]
    fn client_hint_bitrate_is_off_by_default_and_in_old_files() {
        assert!(!Settings::default().client_hint_bitrate);
        let old: Settings = serde_json::from_str(r#"{"fps":60,"adaptive_bitrate":true}"#).unwrap();
        assert!(!old.client_hint_bitrate);
        let on: Settings = serde_json::from_str(r#"{"client_hint_bitrate":true}"#).unwrap();
        assert!(on.client_hint_bitrate);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
        assert!(back.client_hint_bitrate);
    }

    #[test]
    fn the_link_test_before_the_first_bitrate_is_off_by_default_and_in_old_files() {
        assert!(!Settings::default().link_probe);
        let old: Settings = serde_json::from_str(r#"{"fps":60,"bitrate_kbps":0}"#).unwrap();
        assert!(!old.link_probe);
        let on: Settings = serde_json::from_str(r#"{"link_probe":true}"#).unwrap();
        assert!(on.link_probe);
    }

    #[test]
    fn instant_input_and_stream_priority_are_on_by_default_and_round_trip() {
        let defaults = Settings::default();
        assert!(defaults.unbatched_input && defaults.network_priority);
        // A settings file from before the fields existed turns both on, as Android does.
        let old: Settings = serde_json::from_str(r#"{"fps":60,"network_grace_ms":5000}"#).unwrap();
        assert!(old.unbatched_input && old.network_priority);
        let off = Settings { unbatched_input: false, network_priority: false, ..Settings::default() };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&off).unwrap()).unwrap();
        assert_eq!(back, off);
        assert_eq!(serde_json::from_str::<Settings>(&serde_json::to_string(&defaults).unwrap()).unwrap(), defaults);
    }

    #[test]
    fn calibration_follows_the_route_to_the_host_not_the_internet_connection() {
        use crate::network::{Fallback, Network, Toward};
        let wired = PaceCalibration { pace_mbps: 900, client_link_mbps: 2500, host_link_mbps: 2500,
            wifi: false, ssid: None, qualified: true };
        // Wi-Fi carries the internet, but the PC is on the local Ethernet segment.
        let wifi = Network { wifi: true, ssid: Some("Home".into()), link_mbps: 866, ..Default::default() };
        let route = wifi.toward(Ok(Toward { wireless: false, link_mbps: 2500 }));
        assert_eq!(wired.for_network(&route, 2500), 900);
        // A Wi-Fi calibration stays found when the route to the PC is the Wi-Fi.
        let on_wifi = PaceCalibration { pace_mbps: 400, client_link_mbps: 866, host_link_mbps: 2500,
            wifi: true, ssid: Some("Home".into()), qualified: true };
        let route = wifi.toward(Ok(Toward { wireless: true, link_mbps: 0 }));
        assert_eq!(route.link_mbps, 866);
        assert_eq!(on_wifi.for_network(&route, 2500), 400);
        // Without a route answer the internet connection decides, as before.
        assert_eq!(wired.for_network(&wifi.toward(Err(Fallback::NotTelling)), 2500), 0);
    }

    #[test]
    fn calibration_is_used_only_on_the_measured_network_and_link() {
        let mut network = crate::network::Network { wifi: true, ssid: Some("Home".into()), link_mbps: 1200, ..Default::default() };
        let calibration = PaceCalibration { pace_mbps: 900, client_link_mbps: 1200, host_link_mbps: 2500,
            wifi: true, ssid: Some("Home".into()), qualified: true };
        assert_eq!(calibration.for_network(&network, 2500), 900);
        assert_eq!(calibration.for_network(&network, 1000), 0);
        network.ssid = Some("Hotel".into());
        assert_eq!(calibration.for_network(&network, 2500), 0);
        network.ssid = None;
        assert_eq!(calibration.for_network(&network, 2500), 0);
        let restored: PaceCalibration = serde_json::from_str(&serde_json::to_string(&calibration).unwrap()).unwrap();
        assert_eq!(restored, calibration);
    }

    #[test]
    fn old_pc_snapshots_and_explicit_false_survive_serialization() {
        let settings = Settings { fps: 90, microphone: false, ..Default::default() };
        let old: Overrides = serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(old.apply(&Settings { fps: 144, microphone: true, ..Default::default() }), settings);
        let mut own: Overrides = serde_json::from_str(r#"{"microphone":false}"#).unwrap();
        assert_eq!(serde_json::to_string(&own).unwrap(), r#"{"microphone":false}"#);
        own.clear(&["microphone"]);
        assert!(own.is_empty());
        assert!(serde_json::from_str::<Overrides>(r#"{"fps":"bad"}"#).is_err());
        assert!(serde_json::from_str::<Overrides>(r#"{"codec":"unknown"}"#).is_err());
    }

    #[test]
    fn the_overlay_keeps_androids_values_and_defaults() {
        let defaults = Settings::default();
        assert!(!defaults.overlay);
        assert_eq!((defaults.overlay_mode, defaults.overlay_position), (OverlayMode::Compact, OverlayCorner::TopLeft));
        let old: Settings = serde_json::from_str(r#"{"overlay":true}"#).unwrap();
        assert_eq!((old.overlay, old.overlay_mode, old.overlay_position), (true, OverlayMode::Compact, OverlayCorner::TopLeft));
        let mode = |value: &str| serde_json::from_value::<OverlayMode>(serde_json::json!(value)).unwrap();
        assert_eq!(["compact", "statistics", "debug"].map(mode), OverlayMode::ALL);
        // Android's normalizeMode: the old names, and anything else reads as Compact.
        assert_eq!(["advanced", "expanded", "newer"].map(mode), [OverlayMode::Debug, OverlayMode::Statistics, OverlayMode::Compact]);
        let corner = |value: &str| serde_json::from_value::<OverlayCorner>(serde_json::json!(value)).unwrap();
        assert_eq!(["top_left", "top_right", "bottom_left", "bottom_right"].map(corner), OverlayCorner::ALL);
        assert_eq!(corner("middle"), OverlayCorner::TopLeft);
        let set = Settings { overlay_mode: OverlayMode::Debug, overlay_position: OverlayCorner::BottomRight, ..Default::default() };
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!((&json["overlay_mode"], &json["overlay_position"]), (&serde_json::json!("debug"), &serde_json::json!("bottom_right")));
    }

    #[test]
    fn quick_resume_and_input_only_on_any_pc_are_off_by_default_and_in_old_files() {
        let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
        assert!(!old.quick_resume && !old.input_only_any_host);
        assert!(!Settings::default().quick_resume && !Settings::default().input_only_any_host);
        let on: Settings = serde_json::from_str(r#"{"quick_resume":true,"input_only_any_host":true}"#).unwrap();
        assert!(on.quick_resume && on.input_only_any_host);
    }

    #[test]
    fn home_view_keeps_the_rows_per_store_until_chosen_and_reads_back() {
        let old: Settings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
        assert_eq!((old.home_layout, old.home_group, old.home_card_size), (HomeLayout::Rows, HomeGroup::Store, CardSize::Medium));
        assert_eq!(Settings::default().home_layout, HomeLayout::Rows);
        let set = Settings { home_layout: HomeLayout::Grid, home_group: HomeGroup::None, home_card_size: CardSize::Large, ..Default::default() };
        let json = serde_json::to_value(&set).unwrap();
        // The words Android stores for the same choices.
        assert_eq!(
            (&json["home_layout"], &json["home_group"], &json["home_card_size"]),
            (&serde_json::json!("grid"), &serde_json::json!("none"), &serde_json::json!("large"))
        );
        assert_eq!(serde_json::from_value::<Settings>(json).unwrap(), set);
        // By type is Android's "type" too.
        let by_collection: Settings = serde_json::from_str(r#"{"home_group":"collection"}"#).unwrap();
        assert_eq!(by_collection.home_group, HomeGroup::Collection);
        let by_type: Settings = serde_json::from_str(r#"{"home_group":"type"}"#).unwrap();
        assert_eq!(by_type.home_group, HomeGroup::Type);
        assert_eq!(serde_json::to_value(by_type).unwrap()["home_group"], serde_json::json!("type"));
        // A value from a newer Nightjar keeps the default instead of dropping every setting.
        let newer: Settings = serde_json::from_str(r#"{"home_layout":"carousel","home_card_size":"huge","fps":90}"#).unwrap();
        assert_eq!((newer.home_layout, newer.home_card_size, newer.fps), (HomeLayout::Rows, CardSize::Medium, 90));
        assert!(CardSize::Small.scale() < CardSize::Medium.scale() && CardSize::Medium.scale() < CardSize::Large.scale());
    }

    #[test]
    fn quit_on_leave_is_kept_per_game_and_a_known_uuid_decides_alone() {
        let mut record = HostRecord::default();
        assert!(!record.quits_on_leave("game", 5));
        record.set_quit_on_leave("game", 5, true);
        assert_eq!(record.quit_on_leave.iter().map(String::as_str).collect::<Vec<_>>(), ["id:5", "uuid:game"]);
        assert!(record.quits_on_leave("game", 5) && record.quits_on_leave("game", 9));
        assert!(record.quits_on_leave("", 5), "a host without UUIDs matches the ID");
        assert!(!record.quits_on_leave("other", 5), "a reused ID with another UUID does not");
        record.set_quit_on_leave("", 0, true);
        assert_eq!(record.quit_on_leave.len(), 2, "no ID and no UUID keeps nothing");
        assert!(!record.quits_on_leave("", 0));
        let restored: HostRecord = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(restored, record);
        record.set_quit_on_leave("game", 5, false);
        assert!(record.quit_on_leave.is_empty() && !record.quits_on_leave("", 5));
        let old: HostRecord = serde_json::from_str(r#"{"name":"PC"}"#).unwrap();
        assert!(old.quit_on_leave.is_empty());
    }

    #[test]
    fn files_are_replaced_whole_and_only_the_user_reads_them() {
        let dir = std::env::temp_dir().join(format!("nightjar-private-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let store = Store::at(dir.clone()).unwrap();
        let identity = Identity { unique_id: "id".into(), certificate_pem: "cert".into(), private_key_pem: "key".into() };
        store.save_identity(&identity).unwrap();
        store.save_identity(&identity).unwrap();
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["identity.json"], "no temporary file is left behind");
        assert!(store.identity().is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("identity.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the private key is the user's alone");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    /// Settings saved before "Sharper colored text", the shared value lists and the merged
    /// Controllers choice read on, in every layer, and a second load changes nothing.
    #[test]
    fn earlier_settings_read_as_the_merged_choices_in_every_layer() {
        let dir = std::env::temp_dir().join(format!("nightjar-merged-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let store = Store::at(dir.clone()).unwrap();
        // Main settings: only "Prefer YUV 4:4:4" on, a 12-second wait, desktop's old stepper floors.
        std::fs::write(dir.join("settings.json"), r#"{"prefer_yuv444":true,"network_grace_ms":12000,"quality_min_short_edge":256,"quality_min_fps":75,"controllers":false,"mic_gain":1000}"#).unwrap();
        let settings = store.settings();
        assert!(settings.prefer_yuv444 && settings.experimental_packed_444, "either on reads as Automatic");
        assert_eq!((settings.network_grace_ms, settings.quality_min_short_edge, settings.quality_min_fps), (15_000, 360, 60));
        assert_eq!(settings.mic_gain, 400);
        assert!(!settings.controllers && settings.multi_controller, "Controllers off stays Off; a player each comes back with it");
        store.save_settings(&settings).unwrap();
        assert_eq!(store.settings(), settings, "a second load changes nothing");
        // The defaults stay as they were: 3 s reads as Off and is kept.
        let defaults = Settings::default();
        let mut migrated = defaults.clone();
        migrated.migrate();
        assert_eq!(migrated, defaults);
        assert_eq!((defaults.network_grace_ms, defaults.quality_min_short_edge, defaults.quality_min_fps), (3000, 720, 30));
        assert!(!defaults.prefer_yuv444 && !defaults.experimental_packed_444 && defaults.controllers && defaults.multi_controller && defaults.sops);
        // A PC's, a game's and a profile's own values: either key gets both, neither stays unset.
        let layer = |json: &str| -> Overrides { serde_json::from_str(json).unwrap() };
        let packed = layer(r#"{"experimental_packed_444":true}"#);
        assert!(packed.has(&["prefer_yuv444"]) && packed.apply(&Settings::default()).prefer_yuv444);
        let off = layer(r#"{"prefer_yuv444":false}"#);
        let on = Settings { prefer_yuv444: true, experimental_packed_444: true, ..Settings::default() };
        let applied = off.apply(&on);
        assert!(!applied.prefer_yuv444 && !applied.experimental_packed_444, "a layer's Off wins over the main Automatic");
        let neither = layer(r#"{"fps":120}"#);
        assert!(!neither.has(&SHARP_COLOR_KEYS) && neither.apply(&on).experimental_packed_444, "inherited");
        assert_eq!(layer(r#"{"network_grace_ms":5000}"#).apply(&Settings::default()).network_grace_ms, 5000, "Off as stored");
        assert_eq!(layer(r#"{"quality_min_fps":15}"#).apply(&Settings::default()).quality_min_fps, 30);
        let hosts_file = serde_json::json!([{"name": "Desk", "settings": {"experimental_packed_444": true}, "app_settings": {"id:7": {"prefer_yuv444": true, "experimental_packed_444": false}}}]);
        std::fs::write(dir.join("hosts.json"), hosts_file.to_string()).unwrap();
        let hosts = store.hosts();
        let own = hosts[0].settings.as_ref().unwrap();
        assert_eq!(serde_json::to_value(own).unwrap(), serde_json::json!({"experimental_packed_444": true, "prefer_yuv444": true}));
        let game = hosts[0].app_settings("", 7).unwrap();
        assert_eq!(serde_json::to_value(game).unwrap(), serde_json::json!({"experimental_packed_444": true, "prefer_yuv444": true}));
        std::fs::write(dir.join("profiles.json"), r#"{"entries":[{"id":1,"name":"Travel","settings":{"prefer_yuv444":true,"network_grace_ms":40000}}]}"#).unwrap();
        let profile = &store.profiles().entries[0].settings;
        assert_eq!(serde_json::to_value(profile).unwrap(), serde_json::json!({"prefer_yuv444": true, "experimental_packed_444": true, "network_grace_ms": 30000}));
        std::fs::remove_dir_all(dir).ok();
    }
}
