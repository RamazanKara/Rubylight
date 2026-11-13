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

/// Where Nightjar shows itself on Linux (`kms`): `Automatic` opens a window on the desktop, or,
/// with no Wayland or X11 session (a console or kiosk box), draws straight to the display through
/// DRM KMS; `Desktop` and `Kms` choose one. `--kms` and `--no-kms` choose for one start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayOutput {
    #[default]
    Automatic,
    Desktop,
    Kms,
}

impl DisplayOutput {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // the menu is drawn only on Linux
    pub const ALL: [Self; 3] = [Self::Automatic, Self::Desktop, Self::Kms];

    pub fn label(self) -> &'static str {
        match self { Self::Automatic => en("Automatic"), Self::Desktop => en("Desktop"), Self::Kms => en("Straight to the display") }
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
    /// Real VRR: with VRR, show each frame at the PC's frame time plus a steady delay. On by
    /// default: on screen it put 97% of frames within 1 ms of the game's cadence (81 to 90% shown
    /// when decoded) for 1.3 to 1.5 ms, and over Wi-Fi halved the frames off it for 2.6 to 4.2 ms
    /// (docs/pyrowave-decode.md, "Real VRR").
    pub present_at_host_times: bool,
    /// Tear-free VRR (Windows, with `vrr`): a frame that arrives while the display is still drawing
    /// the last one waits for it to finish, at most one refresh, and is shown whole
    /// (`present::VrrSync::Scan`). On by default; off shows every frame at once, with tearing.
    pub vrr_tear_free: bool,
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
    /// What every load does to settings saved by an earlier version. Idempotent: it needs no flags
    /// but `auto_reconnect_default_on`'s and `real_vrr_default_on`'s.
    pub(crate) fn migrate(&mut self) {
        // Turns Recover interrupted streams on once in settings saved before it became the default.
        if !self.auto_reconnect_default_on {
            (self.auto_reconnect, self.auto_reconnect_default_on) = (true, true);
        }
        // The same for Real VRR: its off was the old default, not a choice.
        if !self.real_vrr_default_on {
            (self.present_at_host_times, self.real_vrr_default_on) = (true, true);
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
            present_at_host_times: true,
            vrr_tear_free: true,
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
            display_output: DisplayOutput::Automatic,
            video_planes: true,
            high_performance_gpu_power: false,
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
            internet_play: true,
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
            real_vrr_default_on: true,
            kept_settings_checked: false,
            virtual_display: false,
            virtual_display_scale: 100,
            virtual_display_dpi: 0,
            host_display: HostDisplay::HostDefault,
            error_correction: ErrorCorrection::Host,
            rtx_hdr: None,
            rtx_video: None,
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
    /// The PC's `/internet-play/v1` reply as it came, read on its network while paired: its
    /// rendezvous secret for this device, IPv6 addresses and forwarded ports, for reaching it
    /// from outside (`nightjar_core::internet`). None until a PC that offers it answered.
    pub internet_play: Option<String>,
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
        nightjar_core::internet::set_enabled(settings.internet_play);
        settings
    }

    pub fn save_settings(&self, settings: &Settings) -> std::io::Result<()> {
        // Every change of the settings passes here: the tunnels follow Play over the internet.
        nightjar_core::internet::set_enabled(settings.internet_play);
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
mod tests;
