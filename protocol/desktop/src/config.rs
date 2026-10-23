//! What Nightjar remembers between runs: the client identity, the paired hosts with their
//! pinned certificates, and the settings. JSON files in the platform config directory
//! (`%APPDATA%\Nightjar` on Windows, `~/.config/nightjar` on Linux).

use std::path::{Path, PathBuf};
use std::collections::BTreeMap;

use nightjar_core::http::native_api::library::Library;
use serde::{Deserialize, Serialize};

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
        match self { Self::Fit => "Fit", Self::Fill => "Fill (crop)", Self::Stretch => "Stretch", Self::Native => "Native pixels (1:1)" }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalogScrolling {
    Off,
    #[default]
    Right,
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecoderPreference {
    Auto,
    Software,
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
}

impl Upscaling {
    pub fn label(self) -> &'static str {
        match self { Self::Bilinear => "Bilinear", Self::Fsr1 => "FSR 1", Self::Sgsr1 => "SGSR 1" }
    }
}

impl QualityMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Motion => "Prefer motion",
            Self::Detail => "Prefer detail",
            Self::Battery => "Save power",
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
    pub adaptive_quality: QualityMode,
    pub quality_min_short_edge: u32,
    pub quality_min_fps: u32,
    pub codec: CodecPreference,
    pub hdr: bool,
    pub experimental_packed_444: bool,
    pub audio: AudioChannels,
    pub play_audio_on_host: bool,
    pub window_mode: WindowMode,
    pub video_scaling: VideoScaling,
    pub align_video_top: bool,
    /// The display has variable refresh (G-Sync, FreeSync, HDMI VRR).
    pub vrr: bool,
    /// With VRR: show each frame at the PC's frame time plus a steady delay.
    pub present_at_host_times: bool,
    /// Without VRR: wait for the display's refresh (no tearing).
    pub vsync: bool,
    pub overlay: bool,
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
    pub controller_mouse: bool,
    pub analog_scrolling: AnalogScrolling,
    pub mouse_fixed_step: bool,
    pub gamepad_touchpad_as_mouse: bool,
    pub disable_rumble: bool,
    /// Keep sending controller input while the window is in the background.
    pub background_controllers: bool,
    pub decoder: DecoderPreference,
    pub microphone: bool,
    pub cursor_shapes: bool,
    pub network_grace_ms: u32,
    pub auto_reconnect: bool,
    pub virtual_display: bool,
    pub virtual_display_scale: u32,
    pub host_display: HostDisplay,
    pub full_range: bool,
    pub upscaling: Upscaling,
    pub upscale_sharpness: u32,
    /// Open on Home (Continue playing, the library across PCs) when a paired PC has the
    /// Rubylight native API. Off starts on the list of PCs. Not a per-PC or per-game override.
    pub console_home: bool,
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

impl Default for Settings {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            fps: 0,
            bitrate_kbps: 0,
            adaptive_bitrate: false,
            adaptive_quality: QualityMode::Off,
            quality_min_short_edge: 720,
            quality_min_fps: 30,
            codec: CodecPreference::Auto,
            hdr: false,
            experimental_packed_444: false,
            audio: AudioChannels::Stereo,
            play_audio_on_host: false,
            window_mode: WindowMode::Borderless,
            video_scaling: VideoScaling::Fit,
            align_video_top: false,
            vrr: true,
            present_at_host_times: true,
            vsync: true,
            overlay: false,
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
            controller_mouse: true,
            analog_scrolling: AnalogScrolling::Right,
            mouse_fixed_step: false,
            gamepad_touchpad_as_mouse: false,
            disable_rumble: false,
            background_controllers: false,
            decoder: DecoderPreference::Auto,
            microphone: false,
            cursor_shapes: true,
            network_grace_ms: 3000,
            auto_reconnect: false,
            virtual_display: false,
            virtual_display_scale: 100,
            host_display: HostDisplay::HostDefault,
            full_range: false,
            upscaling: Upscaling::Bilinear,
            upscale_sharpness: 50,
            console_home: true,
        }
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
    /// The certificate pinned at pairing (PEM); None while unpaired.
    pub server_certificate: Option<String>,
    pub mac: Option<String>,
    /// Added by hand rather than found on the network.
    pub manual: bool,
    pub settings: Option<Overrides>,
    pub app_settings: BTreeMap<String, Overrides>,
}

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
        let s = self.apply(&Settings::default());
        s.experimental_packed_444 || s.hdr || s.codec.is_pyrowave() || s.upscaling != Upscaling::Bilinear
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
        if name.is_empty() || name.chars().count() > 32 { return Err("Use a name between 1 and 32 characters."); }
        if self.entries.iter().any(|p| Some(p.id) != except && p.name.to_lowercase() == name.to_lowercase()) {
            return Err("A profile already uses that name.");
        }
        Ok(())
    }

    pub fn create(&mut self, name: &str, settings: Overrides) -> Result<u64, &'static str> {
        self.check_name(name, None)?;
        if self.entries.len() >= 50 { return Err("You can save up to 50 profiles."); }
        let id = self.entries.iter().map(|p| p.id).max().unwrap_or(0).checked_add(1).ok_or("No profile IDs available.")?;
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

impl Store {
    /// The platform config directory, or `NIGHTJAR_CONFIG_DIR` (used by tests and the
    /// end-to-end harness).
    pub fn open() -> std::io::Result<Self> {
        let dir = match std::env::var_os("NIGHTJAR_CONFIG_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => directories::ProjectDirs::from("", "", "Nightjar")
                .map(|d| d.config_dir().to_path_buf())
                .ok_or_else(|| std::io::Error::other("no config directory"))?,
        };
        Self::at(dir)
    }

    pub fn at(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn settings(&self) -> Settings {
        read_json(&self.dir.join("settings.json")).unwrap_or_default()
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
        assert!(store.settings().present_at_host_times);
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
}
