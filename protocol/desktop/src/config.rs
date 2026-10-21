//! What Nightjar remembers between runs: the client identity, the paired hosts with their
//! pinned certificates, and the settings. JSON files in the platform config directory
//! (`%APPDATA%\Nightjar` on Windows, `~/.config/nightjar` on Linux).

use std::path::{Path, PathBuf};
use std::collections::BTreeMap;

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
    /// The display has variable refresh (G-Sync, FreeSync, HDMI VRR).
    pub vrr: bool,
    /// With VRR: show each frame at the PC's frame time plus a steady delay.
    pub present_at_host_times: bool,
    /// Without VRR: wait for the display's refresh (no tearing).
    pub vsync: bool,
    pub overlay: bool,
    pub mouse_mode: MouseMode,
    /// Send Alt+Tab, the Windows key and similar to the host while the stream has focus.
    pub capture_system_keys: bool,
    pub controllers: bool,
    /// Nintendo layout: swap A/B and X/Y.
    pub swap_face_buttons: bool,
    pub controller_motion: bool,
    pub controller_audio: bool,
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
            vrr: true,
            present_at_host_times: true,
            vsync: true,
            overlay: false,
            mouse_mode: MouseMode::Capture,
            capture_system_keys: true,
            controllers: true,
            swap_face_buttons: false,
            controller_motion: true,
            controller_audio: false,
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
    /// The host's `uniqueid`; empty until the host answered once.
    pub unique_id: String,
    pub name: String,
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

impl HostRecord {
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
}

impl Profiles {
    pub fn settings(&self, base: &Settings) -> Settings {
        self.entries.iter().find(|p| Some(p.id) == self.active).map_or_else(|| base.clone(), |p| p.settings.apply(base))
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
    }
}

pub struct Store {
    dir: PathBuf,
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

    /// Box art cache: `art/<host>/<app>.img`.
    pub fn art_path(&self, host: &str, app_id: i32) -> PathBuf {
        let host: String = host
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        self.dir.join("art").join(host).join(format!("{app_id}.img"))
    }
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
    fn partial_profiles_and_game_overrides_follow_their_layers() {
        let mut global = Settings { fps: 120, microphone: true, ..Default::default() };
        let mut profiles = Profiles::default();
        let id = profiles.create(" Home ", serde_json::from_str(r#"{"codec":"Av1","fps":90}"#).unwrap()).unwrap();
        profiles.active = Some(id);
        assert!(profiles.create("HOME", Overrides::default()).is_err());
        let mut host = HostRecord { settings: Some(serde_json::from_str(r#"{"fps":60}"#).unwrap()), ..Default::default() };
        host.save_app_settings("game-a", 1, serde_json::from_str(r#"{"hdr":true,"microphone":false}"#).unwrap());
        global.upscale_sharpness = 80;
        let base = host.settings(&profiles.settings(&global));
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
        assert_eq!(profiles.settings(&global), global);
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
