//! What Nightjar remembers between runs: the client identity, the paired hosts with their
//! pinned certificates, and the settings. JSON files in the platform config directory
//! (`%APPDATA%\Nightjar` on Windows, `~/.config/nightjar` on Linux).

use std::path::{Path, PathBuf};
use std::collections::{BTreeMap, BTreeSet};

use nightjar_core::http::native_api::library::Library;
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
        match self { Self::Fit => en("Fit"), Self::Fill => en("Fill (crop)"), Self::Stretch => en("Stretch"), Self::Native => en("Native pixels (1:1)") }
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

impl GamepadQuitCombo {
    pub const ALL: [Self; 6] = [Self::StartSelectLbRb, Self::SelectLbRbY, Self::StartLbRbA, Self::StartLbRbB, Self::LbRbXY, Self::LbRbAB];

    /// Button names, the same in every language.
    pub fn label(self) -> &'static str {
        match self {
            Self::StartSelectLbRb => "Start+Select+LB+RB",
            Self::SelectLbRbY => "Select+LB+RB+Y",
            Self::StartLbRbA => "Start+LB+RB+A",
            Self::StartLbRbB => "Start+LB+RB+B",
            Self::LbRbXY => "LB+RB+X+Y",
            Self::LbRbAB => "LB+RB+A+B",
        }
    }
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
    Cpu,
    #[default]
    Gpu,
}

impl PyroWaveDecoder {
    pub fn label(self) -> &'static str {
        match self { Self::Cpu => en("CPU"), Self::Gpu => en("GPU") }
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
    pub experimental_packed_444: bool,
    /// "Prefer YUV 4:4:4" (Experimental): offers the 8-bit 4:4:4 formats (`color::offer`).
    pub prefer_yuv444: bool,
    /// "10-bit color (SDR)" (Experimental): offers Main10 without asking for HDR.
    pub sdr_10bit: bool,
    pub audio: AudioChannels,
    pub play_audio_on_host: bool,
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
    /// The performance overlay is on (Android's `checkbox_enable_perf_overlay`), showing
    /// `overlay_mode` in the `overlay_position` corner. The stream menu and its shortcut change
    /// these and keep them.
    pub overlay: bool,
    pub overlay_mode: OverlayMode,
    pub overlay_position: OverlayCorner,
    pub mouse_mode: MouseMode,
    pub mouse_speed: u32,
    pub mouse_nav_buttons: bool,
    pub right_alt_as_meta: bool,
    /// Send Alt+Tab, the Windows key and similar to the host while the stream has focus.
    pub capture_system_keys: bool,
    pub controllers: bool,
    /// Nintendo layout: swap A/B and X/Y.
    pub swap_face_buttons: bool,
    pub controller_motion: bool,
    pub controller_audio: bool,
    pub back_opens_menu: bool,
    pub gamepad_quit_combo: GamepadQuitCombo,
    pub controller_mouse: bool,
    pub analog_scrolling: AnalogScrolling,
    pub mouse_fixed_step: bool,
    pub gamepad_touchpad_as_mouse: bool,
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
    pub microphone: bool,
    pub cursor_shapes: bool,
    pub network_grace_ms: u32,
    /// Client roaming (Experimental): keep the stream when this device changes networks, with a
    /// Rubylight PC that supports it. On by default.
    pub client_roaming: bool,
    /// Let client roaming move the stream onto a metered network. Off by default.
    pub roaming_metered: bool,
    /// USB forwarding (Experimental): attach USB devices plugged in here to the PC from the
    /// stream controls (src/usb.rs). Off by default.
    pub usb_forwarding: bool,
    /// Clipboard sync: what is copied here goes on the PC's clipboard and the PC's copies come
    /// here, as text, with a Rubylight PC that allows this device its clipboard (0x5508,
    /// stream/clipboard_sync.rs). Off by default.
    pub clipboard_sync: bool,
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
    /// "Prioritize stream traffic": socket and thread priority for the stream, which the engine
    /// sets on Linux only. On by default, as on Android.
    pub network_priority: bool,
    /// "Recover interrupted streams". On by default, as on Android.
    pub auto_reconnect: bool,
    /// The once-only switch of `auto_reconnect` to on has run (Android's
    /// `auto_reconnect_default_on`). Missing from files written before it, so those are switched
    /// once; a choice made after that is kept.
    #[serde(default)]
    pub auto_reconnect_default_on: bool,
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
    pub full_range: bool,
    pub upscaling: Upscaling,
    pub upscale_sharpness: u32,
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
    /// The launcher's size in percent: one of [`UI_ZOOM_STEPS`]. Not a per-PC or per-game override.
    pub ui_zoom: u32,
    /// Brighter text, black surfaces and thicker outlines and focus rings.
    pub high_contrast: bool,
    pub reduce_motion: ReduceMotion,
    pub language: Language,
    /// Windows: look for a newer stable release once per launch (update_check.rs).
    pub check_updates: bool,
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
    /// The system's language when Nightjar has it, else English; also what an unknown value
    /// from a newer Nightjar reads as. Keep it last: serde wants the catch-all there.
    #[default]
    #[serde(other)]
    System,
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

/// The video FEC this client asks for (`rlFecPercentage`): the PC's own, automatic from this
/// device's loss reports, or a fixed share. Other PCs keep their own.
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

impl Settings {
    /// Turns Recover interrupted streams on once in settings saved before it became the default.
    fn migrate(&mut self) {
        if !self.auto_reconnect_default_on {
            (self.auto_reconnect, self.auto_reconnect_default_on) = (true, true);
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
            experimental_packed_444: false,
            prefer_yuv444: false,
            sdr_10bit: false,
            audio: AudioChannels::Stereo,
            play_audio_on_host: false,
            audio_passthrough: false,
            window_mode: WindowMode::Borderless,
            video_scaling: VideoScaling::Fit,
            align_video_top: false,
            vrr: true,
            present_at_host_times: false,
            vsync: true,
            overlay: false,
            overlay_mode: OverlayMode::Compact,
            overlay_position: OverlayCorner::TopLeft,
            mouse_mode: MouseMode::Capture,
            mouse_speed: 100,
            mouse_nav_buttons: true,
            right_alt_as_meta: false,
            capture_system_keys: true,
            controllers: true,
            swap_face_buttons: false,
            controller_motion: true,
            controller_audio: false,
            back_opens_menu: false,
            gamepad_quit_combo: GamepadQuitCombo::StartSelectLbRb,
            controller_mouse: true,
            analog_scrolling: AnalogScrolling::Right,
            mouse_fixed_step: false,
            gamepad_touchpad_as_mouse: false,
            disable_rumble: false,
            background_controllers: false,
            decoder: DecoderPreference::Auto,
            early_decode: false,
            beam_race: false,
            beam_race_margin_ms: 0,
            pyrowave_decoder: PyroWaveDecoder::default(),
            microphone: false,
            cursor_shapes: true,
            network_grace_ms: 3000,
            client_roaming: true,
            roaming_metered: false,
            usb_forwarding: false,
            clipboard_sync: false,
            tell_host_wireless: true,
            tell_host_link_speed: true,
            unbatched_input: true,
            network_priority: true,
            auto_reconnect: true,
            auto_reconnect_default_on: true,
            virtual_display: false,
            virtual_display_scale: 100,
            virtual_display_dpi: 0,
            host_display: HostDisplay::HostDefault,
            error_correction: ErrorCorrection::Host,
            link_probe: false,
            full_range: false,
            upscaling: Upscaling::Bilinear,
            upscale_sharpness: 50,
            console_home: true,
            quick_resume: false,
            input_only_any_host: false,
            ui_zoom: 100,
            high_contrast: false,
            reduce_motion: ReduceMotion::System,
            language: Language::System,
            check_updates: true,
        }
    }
}

impl Settings {
    /// Whether these settings draw through the GPU renderer, which Linux picks at start: SDL's
    /// "gpu" renderer runs the shader passes (packed full chroma, HDR, PyroWave, upscaling, and
    /// the plane pass that 4:4:4 and 10-bit pictures need).
    pub fn needs_gpu(&self) -> bool {
        self.experimental_packed_444 || self.hdr || self.codec.is_pyrowave() || self.upscaling != Upscaling::Bilinear
            || self.prefer_yuv444 || self.sdr_10bit
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
    /// The known address the user chose to always connect through, or None for the automatic
    /// pick (`address` first, then `other_addresses`), as Moonlight V+ PC's `pinnedAddress`.
    pub pinned_address: Option<String>,
    /// The certificate pinned at pairing (PEM); None while unpaired.
    pub server_certificate: Option<String>,
    pub mac: Option<String>,
    /// The MACs of the PC's other adapters and the subnet broadcasts that reach them, from its
    /// native `GET /wake` (`wol::targets`); kept so a PC that sleeps can be woken through them.
    pub wake_macs: Vec<String>,
    pub wake_broadcasts: Vec<String>,
    /// Added by hand rather than found on the network.
    pub manual: bool,
    pub settings: Option<Overrides>,
    pub app_settings: BTreeMap<String, Overrides>,
    /// The games whose stream quits them on the PC when the player leaves it, kept as Android's
    /// `AppQuitOnLeave` does: under `uuid:<uuid>` and `id:<id>`.
    pub quit_on_leave: BTreeSet<String>,
    /// The once-per-PC notices already shown for this PC, by Android's `HostNotices` names.
    pub notices: BTreeSet<String>,
}

/// PyroWave was chosen and a stream from this PC ran another codec.
pub const NOTICE_PYROWAVE_FALLBACK: &str = "PYROWAVE_FALLBACK";

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

fn app_key(uuid: &str, id: i32) -> String {
    if uuid.is_empty() { format!("id:{id}") } else { format!("uuid:{uuid}") }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Overrides(serde_json::Map<String, serde_json::Value>);

impl<'de> Deserialize<'de> for Overrides {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = serde_json::Map::deserialize(deserializer)?;
        serde_json::from_value::<Settings>(serde_json::Value::Object(values.clone())).map_err(serde::de::Error::custom)?;
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
        self.entries.iter().find(|p| Some(p.id) == self.selected(network)).map_or_else(|| base.clone(), |p| p.settings.apply(base))
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

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    // Write and rename, so a crash never leaves half a file.
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
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
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn settings(&self) -> Settings {
        let mut settings: Settings = read_json(&self.dir.join("settings.json")).unwrap_or_default();
        settings.migrate();
        settings
    }

    pub fn save_settings(&self, settings: &Settings) -> std::io::Result<()> {
        write_json(&self.dir.join("settings.json"), settings)
    }

    pub fn profiles(&self) -> Profiles {
        read_json(&self.dir.join("profiles.json")).unwrap_or_default()
    }

    pub fn save_profiles(&self, profiles: &Profiles) -> std::io::Result<()> {
        write_json(&self.dir.join("profiles.json"), profiles)
    }

    pub fn controller_mappings(&self) -> BTreeMap<String, crate::gamepad::Mapping> {
        let mut mappings: BTreeMap<String, crate::gamepad::Mapping> = read_json(&self.dir.join("controllers.json")).unwrap_or_default();
        mappings.retain(|_, mapping| mapping.valid());
        mappings
    }

    pub fn shortcuts(&self) -> crate::shortcuts::Shortcuts {
        let path = self.dir.join("shortcuts.json");
        if !path.exists() { return crate::shortcuts::Shortcuts::default(); }
        crate::shortcuts::Shortcuts::read(&path).unwrap_or_else(|e| {
            eprintln!("nightjar: ignoring {}: {e}", path.display());
            crate::shortcuts::Shortcuts::default()
        })
    }

    pub fn window_placement(&self) -> Option<crate::window_place::Placement> {
        read_json(&self.dir.join("window.json"))
    }

    pub fn save_window_placement(&self, placement: &crate::window_place::Placement) -> std::io::Result<()> {
        write_json(&self.dir.join("window.json"), placement)
    }

    pub fn save_shortcuts(&self, shortcuts: &crate::shortcuts::Shortcuts) -> std::io::Result<()> {
        write_json(&self.dir.join("shortcuts.json"), shortcuts)
    }

    pub fn save_controller_mappings(&self, mappings: &BTreeMap<String, crate::gamepad::Mapping>) -> std::io::Result<()> {
        write_json(&self.dir.join("controllers.json"), mappings)
    }

    pub fn identity(&self) -> Option<Identity> {
        read_json(&self.dir.join("identity.json"))
    }

    pub fn save_identity(&self, identity: &Identity) -> std::io::Result<()> {
        write_json(&self.dir.join("identity.json"), identity)
    }

    pub fn hosts(&self) -> Vec<HostRecord> {
        read_json(&self.dir.join("hosts.json")).unwrap_or_default()
    }

    pub fn save_hosts(&self, hosts: &[HostRecord]) -> std::io::Result<()> {
        write_json(&self.dir.join("hosts.json"), &hosts)
    }

    pub fn recents(&self) -> Vec<Recent> {
        read_json(&self.dir.join("recent.json")).unwrap_or_default()
    }

    pub fn save_recents(&self, recents: &[Recent]) -> std::io::Result<()> {
        write_json(&self.dir.join("recent.json"), &recents)
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
        write_json(&self.dir.join("favourites.json"), &favourites)
    }

    /// Box art cache: `art/<host>/<app>.img`, or `<app>-<version>.img` for a cover the host
    /// versions (the native API), so a changed cover is fetched again.
    pub fn art_path(&self, host: &str, app_id: i32, version: &str) -> PathBuf {
        let version: String = version.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let name = if version.is_empty() { format!("{app_id}.img") } else { format!("{app_id}-{version}.img") };
        self.dir.join("art").join(sanitize(host)).join(name)
    }

    /// The last native library a PC sent, for Home while the PC is away or not polled yet.
    pub fn library(&self, host: &str) -> Option<Library> {
        read_json(&self.library_path(host))
    }

    pub fn save_library(&self, host: &str, library: &Library) -> std::io::Result<()> {
        let path = self.library_path(host);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_json(&path, library)
    }

    pub fn forget_library(&self, host: &str) {
        let _ = std::fs::remove_file(self.library_path(host));
    }

    /// Everything kept for a PC that is removed: its library, its covers, and its games in the
    /// recent and favourite lists. Returns both lists as they are now.
    pub fn forget_host(&self, host: &str, recents: &[Recent], favourites: &[Favourite]) -> (Vec<Recent>, Vec<Favourite>) {
        self.forget_library(host);
        let _ = std::fs::remove_dir_all(self.dir.join("art").join(sanitize(host)));
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
}

/// After a cover is saved at `keep` (`<app>.img` or `<app>-<version>.img`), removes the other
/// files of the same app in that folder: older versions of the cover are never asked for again.
pub fn prune_art(keep: &Path, app_id: i32) {
    let Some(dir) = keep.parent() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let plain = format!("{app_id}.img");
    let versioned = format!("{app_id}-");
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let same_app = name == plain || (name.starts_with(&versioned) && name.ends_with(".img"));
        if same_app && path != keep {
            let _ = std::fs::remove_file(&path);
        }
    }
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
        let library: Library = serde_json::from_str(include_str!("../../client/tests/fixtures/native-api/library.json")).unwrap();
        let dir = std::env::temp_dir().join(format!("nightjar-library-{}", std::process::id()));
        let store = Store::at(dir.clone()).unwrap();
        let host = "5E0F4B1C-7A3D-4C52-9E10-0B6F2D8A4C11";
        assert!(store.library(host).is_none());
        store.save_library(host, &library).unwrap();
        let back = store.library(host).unwrap();
        assert_eq!((back.revision.as_str(), back.games.len()), ("9f2c41d0", 2));
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
        prune_art(&new, 7);
        assert!(new.exists(), "the cover just saved stays");
        assert!(!old.exists() && !plain.exists(), "older versions of app 7 go");
        assert!(other_app.exists() && other_app_plain.exists(), "other apps keep theirs (70 is not 7)");
        let _ = std::fs::remove_dir_all(dir);
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
        let set: Settings = serde_json::from_str("{\"fps\":90,\"reduce_motion\":\"Sometimes\",\"language\":\"French\",\"ui_zoom\":130}").unwrap();
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
    fn pyrowave_decoder_defaults_to_the_gpu_and_reads_from_the_file() {
        let default = PyroWaveDecoder::Gpu;
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
        shortcuts.save(None, "Task manager", "CTRL + SHIFT + ESC").unwrap();
        store.save_shortcuts(&shortcuts).unwrap();
        assert_eq!(store.shortcuts(), shortcuts);
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
}
