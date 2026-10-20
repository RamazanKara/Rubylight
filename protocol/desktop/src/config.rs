//! What Nightjar remembers between runs: the client identity, the paired hosts with their
//! pinned certificates, and the settings. JSON files in the platform config directory
//! (`%APPDATA%\Nightjar` on Windows, `~/.config/nightjar` on Linux).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodecPreference {
    Auto,
    H264,
    Hevc,
    Av1,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 0x0 is the display's native resolution.
    pub width: u32,
    pub height: u32,
    /// 0 is the display's refresh rate.
    pub fps: u32,
    /// 0 picks a bitrate for the resolution and rate.
    pub bitrate_kbps: u32,
    pub codec: CodecPreference,
    pub hdr: bool,
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
    /// Keep sending controller input while the window is in the background.
    pub background_controllers: bool,
    pub decoder: DecoderPreference,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            fps: 0,
            bitrate_kbps: 0,
            codec: CodecPreference::Auto,
            hdr: false,
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
            background_controllers: false,
            decoder: DecoderPreference::Auto,
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
        let hosts = vec![HostRecord {
            unique_id: "ABC".into(),
            name: "Desk".into(),
            address: "10.0.0.2".into(),
            http_port: 47989,
            server_certificate: Some("PEM".into()),
            ..Default::default()
        }];
        store.save_hosts(&hosts).unwrap();
        assert_eq!(store.hosts(), hosts);
        // A settings file from an older version keeps the new fields' defaults.
        std::fs::write(dir.join("settings.json"), r#"{"fps": 90}"#).unwrap();
        assert_eq!(store.settings().fps, 90);
        assert!(store.settings().present_at_host_times);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
