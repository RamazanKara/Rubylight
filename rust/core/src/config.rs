//! The host configuration: `sunshine.conf`-compatible `key = value` settings,
//! typed accessors with Vibepollo's defaults, per-app and per-client overrides,
//! and the port layout derived from the base port.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

mod settings30;
pub use settings30::{BackgroundMode, DisplayAdjust, SETTINGS_30, SharedFolderConfig, TouchMode};

/// Setting values Rubylight no longer offers, and what replaces each:
/// the ViGEmBus controllers, which Sunshine, Apollo, Vibepollo and earlier
/// Rubylight profiles can carry, became the VHF pad of the same family.
pub const RETIRED_VALUES: &[(&str, &str, &str)] = &[
    ("gamepad", "x360", "vhf_xbox_one"),
    ("gamepad", "ds4", "vhf_ds4"),
];
/// Settings whose values select features Rubylight removed. A value of
/// `key` that `retired` matches is replaced by `current` wherever the host
/// reads settings, and a value read from sunshine.conf is reported once
/// (see [`Config::load_retiring`]). Another setting's removed values take
/// one more row.
pub struct RetiredSetting {
    pub key: &'static str,
    pub retired: fn(&str) -> bool,
    pub current: &'static str,
    /// Why the value no longer applies, for the warning.
    pub reason: &'static str,
}
pub const RETIRED_SETTINGS: &[RetiredSetting] = &[
    RetiredSetting {
        key: "encoder",
        retired: crate::encoder_policy::retired,
        current: "amf",
        reason: "this host encodes H.264, HEVC and AV1 with AMF only",
    },
    RetiredSetting {
        key: "frame_limiter_provider",
        retired: crate::framegen::retired_provider,
        current: "auto",
        reason: "RTSS is the only frame limiter",
    },
    RetiredSetting {
        key: "rtss_frame_limit_type",
        retired: crate::framegen::retired_limit_type,
        current: "async",
        reason: "RTSS's Reflex mode needs an NVIDIA GPU",
    },
    RetiredSetting {
        key: "frame-generation-mode",
        retired: crate::framegen::retired_generation,
        current: "none",
        reason: "driver frame generation needs an NVIDIA GPU",
    },
    RetiredSetting {
        key: "frame-generation-provider",
        retired: crate::framegen::retired_generation,
        current: "none",
        reason: "driver frame generation needs an NVIDIA GPU",
    },
];
/// A retired value the host replaced while reading the configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retired {
    pub key: String,
    pub value: String,
    pub current: &'static str,
    pub reason: &'static str,
}
/// The one-time warning for values replaced in sunshine.conf.
pub fn retired_warning(retired: &[Retired]) -> Option<String> {
    let changes: Vec<_> = retired
        .iter()
        .map(|r| format!("{} {} is now {} ({})", r.key, r.value, r.current, r.reason))
        .collect();
    (!changes.is_empty()).then(|| {
        format!(
            "Saved settings for removed features were changed: {}. The configuration file now holds the new values; check them in Settings.",
            changes.join("; ")
        )
    })
}
pub const HARDWARE_METRICS: &[&str] = &[
    "cpu_usage_percent",
    "gpu_usage_percent",
    "gpu_clock_mhz",
    "gpu_memory_clock_mhz",
    "gpu_temperature_c",
    "gpu_hotspot_temperature_c",
    "gpu_memory_temperature_c",
    "gpu_voltage_mv",
    "gpu_fan_rpm",
    "gpu_fan_percent",
    "gpu_power_w",
    "gpu_board_power_w",
    "gpu_vram_used_mb",
    "gpu_vram_usage_percent",
];
/// Settings that Vibepollo, Apollo and earlier Rubylight releases saved under
/// another name, and the name the host reads now. The values mean the same
/// under both names.
pub const RENAMED_KEYS: &[(&str, &str)] = &[
    ("rtss_path", "rtss_install_path"),
    (
        "dd_vdd_static_monitor_count",
        "dd_virtual_display_permanent_count",
    ),
    // Also what a virtual display reports, the one part of NVIDIA RTX HDR
    // that stays.
    ("rtx_hdr_peak_brightness", "hdr_peak_brightness"),
];
/// Settings of features Rubylight removed: NVIDIA RTX HDR (TrueHDR), the
/// NVIDIA driver's VSYNC override (all the dummy plug workaround still
/// did), and the software encoders' settings. Saved profiles, apps and
/// devices can still carry them; the host drops them without a warning,
/// and the next save leaves them out.
pub const REMOVED_KEYS: &[&str] = &[
    "rtx_hdr",
    "rtx_hdr_sdr_brightness",
    "rtx_hdr_contrast",
    "rtx_hdr_saturation",
    "rtx_hdr_middle_gray",
    "frame_limiter_disable_vsync",
    "rtss_disable_vsync_ullm",
    "dd_wa_dummy_plug_hdr10",
    "min_threads",
    "crf",
    "qp",
];
/// The settings of the removed NVENC, Quick Sync and software encoders.
pub const REMOVED_PREFIXES: &[&str] = &["nvenc_", "qsv_", "sw_"];
/// Whether `key` belongs to a removed feature (`REMOVED_KEYS`,
/// `REMOVED_PREFIXES`).
pub fn removed(key: &str) -> bool {
    REMOVED_KEYS.contains(&key) || REMOVED_PREFIXES.iter().any(|p| key.starts_with(p))
}
/// The name the host reads `key` under now.
pub fn current_name(key: &str) -> &str {
    RENAMED_KEYS
        .iter()
        .find(|(old, _)| *old == key)
        .map_or(key, |(_, new)| new)
}
/// The current value for a retired `value` of `key`, if it is one.
pub fn replacement(key: &str, value: &str) -> Option<&'static str> {
    retirement(key, value).map(|(current, _)| current)
}
/// The current value for a retired `value` of `key`, and why it changed.
fn retirement(key: &str, value: &str) -> Option<(&'static str, &'static str)> {
    let value = unquote(value.trim());
    RETIRED_VALUES
        .iter()
        .find(|(k, old, _)| *k == key && *old == value)
        .map(|(_, _, new)| (*new, "the controller type was replaced"))
        .or_else(|| {
            RETIRED_SETTINGS
                .iter()
                .find(|setting| setting.key == key && (setting.retired)(value))
                .map(|setting| (setting.current, setting.reason))
        })
}

/// Replace retired values among an app's or a device's settings, which are
/// JSON. Returns whether anything changed.
pub fn replace_retired<'a>(
    settings: impl IntoIterator<Item = (&'a String, &'a mut serde_json::Value)>,
) -> bool {
    let mut changed = false;
    for (key, value) in settings {
        if let Some(current) = value.as_str().and_then(|old| replacement(key, old)) {
            *value = serde_json::Value::from(current);
            changed = true;
        }
    }
    changed
}

/// Replace retired values in a library app: its top-level settings, as
/// Apollo and Vibepollo store them, and its `config-overrides`. Returns
/// whether anything changed.
pub fn replace_retired_in_app(app: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let mut changed = replace_retired(app.iter_mut());
    if let Some(overrides) = app
        .get_mut("config-overrides")
        .and_then(serde_json::Value::as_object_mut)
    {
        changed |= replace_retired(overrides.iter_mut());
    }
    changed
}

/// Retain all keys, including extension keys a newer UI or client may write.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub values: BTreeMap<String, String>,
}
impl Config {
    /// Read `sunshine.conf` as Vibepollo does, so an imported file never stops
    /// the host: a line without `=` or with an unusable key is skipped with a
    /// warning, characters before a key (a UTF-8 byte order mark) are dropped,
    /// the first of duplicate keys wins and an empty value keeps the default.
    /// `#` starts a comment outside quotes, and a value opening `[` or `{`
    /// continues until its brackets close. A setting saved under an earlier
    /// name is read under its current one (`RENAMED_KEYS`).
    pub fn parse(text: &str) -> Result<Self> {
        Ok(Self::parse_retiring(text)?.0)
    }
    /// `parse`, and the retired values it replaced (`RETIRED_VALUES`,
    /// `RETIRED_SETTINGS`).
    pub fn parse_retiring(text: &str) -> Result<(Self, Vec<Retired>)> {
        let (mut config, retired) = Self::parse_names_as_written(text)?;
        config.rename_keys();
        Ok((config, retired))
    }
    fn parse_names_as_written(text: &str) -> Result<(Self, Vec<Retired>)> {
        let mut values = BTreeMap::new();
        let mut retired = Vec::new();
        let mut lines = text.lines().enumerate();
        while let Some((number, raw)) = lines.next() {
            let number = number + 1;
            if raw.contains('\0') {
                tracing::warn!(line = number, "configuration line contains NUL; skipped");
                continue;
            }
            let line = strip_comment(raw);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                tracing::warn!(line = number, "configuration line has no '='; skipped");
                continue;
            };
            let key = key
                .trim()
                .trim_start_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
            let mut value = value.trim().to_owned();
            if value.starts_with(['[', '{']) {
                let continuation = lines.clone();
                let mut depth = bracket_depth(&value);
                while depth > 0 {
                    let Some((_, next)) = lines.next() else {
                        break;
                    };
                    let next = strip_comment(next);
                    depth += bracket_depth(&next);
                    value.push('\n');
                    value.push_str(next.trim());
                }
                if depth != 0 {
                    lines = continuation;
                    tracing::warn!(
                        line = number,
                        key,
                        "configuration list is not closed; skipped"
                    );
                    continue;
                }
            }
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                tracing::warn!(line = number, key, "invalid configuration key; skipped");
                continue;
            }
            if value.is_empty() || removed(key) {
                continue;
            }
            if values.contains_key(key) {
                tracing::warn!(
                    line = number,
                    key,
                    "configuration key repeated; the first value is used"
                );
                continue;
            }
            // The console then shows, and the next save writes, the
            // replacement.
            if let Some((current, reason)) = retirement(key, &value) {
                tracing::info!(
                    key,
                    retired = value,
                    current,
                    "retired setting value replaced"
                );
                retired.push(Retired {
                    key: key.to_owned(),
                    value: std::mem::replace(&mut value, current.to_owned()),
                    current,
                    reason,
                });
            }
            values.insert(key.to_owned(), value);
        }
        Ok((Self { values }, retired))
    }
    /// Move settings found under an earlier name to their current one. When
    /// both names are set the current one is what the host has read, so it
    /// stays and the earlier one is dropped. The file keeps the earlier name
    /// until the console next saves, which writes every setting under the
    /// name it is held under here.
    fn rename_keys(&mut self) {
        for &(old, new) in RENAMED_KEYS {
            if let Some(value) = self.values.remove(old)
                && !self.values.contains_key(new)
            {
                self.values.insert(new.to_owned(), value);
            }
        }
    }
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self::load_retiring(path)?.0)
    }
    /// `load`, and the retired values it replaced. The host writes the
    /// replacements back and warns once, so the next start is quiet.
    pub fn load_retiring(path: &Path) -> Result<(Self, Vec<Retired>)> {
        match std::fs::read(path) {
            Ok(bytes) => Self::parse_retiring(&decode(&bytes))
                .with_context(|| format!("reading {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(e.into()),
        }
    }
    /// The value of `key`, or `default` when it is unset or empty.
    pub fn get<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.values
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(default)
    }
    pub fn boolean(&self, key: &str, default: bool) -> bool {
        self.values
            .get(key)
            .and_then(|s| parse_boolean(s))
            .unwrap_or(default)
    }
    /// Whether direct driver push (`idd-push`) uses the GPU fence ring. It is the default;
    /// `idd_push_fence_ring = false` selects the keyed-mutex ring. Capture reuse and capture
    /// itself read this one answer so they cannot disagree.
    pub fn idd_push_fence_ring(&self) -> bool {
        self.boolean("idd_push_fence_ring", true)
    }
    /// How long a disconnected stream may keep its display. Exclusive layouts
    /// must release even when an older profile disables restoration: otherwise
    /// the local user's physical displays can remain off for hours.
    pub fn display_disconnect_delay(&self, exclusive: bool) -> Option<std::time::Duration> {
        if exclusive || self.boolean("dd_config_revert_on_disconnect", true) {
            Some(std::time::Duration::from_millis(
                self.integer("dd_config_revert_delay", 3000).max(0) as u64,
            ))
        } else {
            let seconds = self
                .integer("dd_paused_virtual_display_timeout_secs", 7200)
                .max(0);
            (seconds != 0).then(|| std::time::Duration::from_secs(seconds as u64))
        }
    }
    /// Experimental packet-rate adaptation is explicitly enabled per stream.
    /// Manual pacing and host bitrate adaptation own their respective limits;
    /// Both conventional and PyroWave streams use the loss controller when
    /// `host_adaptive_pacing` is set. A client that reports a Wi-Fi or cellular
    /// route (`client_wireless`) gets it by default on H.264, HEVC and AV1
    /// (codec 0-2), unless `wireless_adaptive_pacing` is off; PyroWave keeps its
    /// calibrated-pace policy.
    pub fn adaptive_packet_pacing(&self, codec: u8, client_wireless: bool) -> bool {
        self.pacing_controller_chosen(codec, client_wireless)
            && self.integer("pacing_max_bitrate_kbps", 0) <= 0
            && !self.boolean("host_adaptive_bitrate", false)
            && codec <= 3
    }
    fn pacing_controller_chosen(&self, codec: u8, client_wireless: bool) -> bool {
        self.boolean("host_adaptive_pacing", false)
            || (client_wireless && codec <= 2 && self.boolean("wireless_adaptive_pacing", true))
    }
    /// Pace like Wi-Fi after a lost keyframe or repeated frame loss (on by default). A
    /// manual pacing limit and the opt-in adaptations keep their own policies,
    /// and so does a stream the adaptive pacing controller owns.
    pub fn keyframe_loss_pacing(&self, codec: u8, client_wireless: bool) -> bool {
        self.boolean("keyframe_loss_pacing", true)
            && self.integer("pacing_max_bitrate_kbps", 0) <= 0
            && !self.pacing_controller_chosen(codec, client_wireless)
            && !self.boolean("host_adaptive_bitrate", false)
    }
    /// Lower the encoder bitrate when the link cannot carry the stream (on by
    /// default). The opt-in `host_adaptive_bitrate` owns the bitrate when set.
    pub fn link_rate_adaptation(&self) -> bool {
        self.boolean("link_rate_adaptation", true) && !self.boolean("host_adaptive_bitrate", false)
    }
    /// An integer setting; Vibepollo also writes quoted (`"756"`) and
    /// hexadecimal (`0x2a`) values.
    pub fn integer(&self, key: &str, default: i64) -> i64 {
        let Some(value) = self.values.get(key).filter(|v| !v.trim().is_empty()) else {
            return default;
        };
        parse_integer(value).unwrap_or_else(|| {
            invalid(key, value);
            default
        })
    }
    /// A list setting. JSON arrays and Vibepollo's `[a, b]` or `a, b` lists
    /// are accepted; numbers are returned as written.
    pub fn list(&self, key: &str) -> Vec<String> {
        let Some(value) = self.values.get(key) else {
            return Vec::new();
        };
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str(value) {
            return items
                .into_iter()
                .map(|item| match item {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                })
                .filter(|s| !s.trim().is_empty())
                .collect();
        }
        let inner = value.trim();
        let inner = inner.strip_prefix('[').unwrap_or(inner);
        let inner = inner.strip_suffix(']').unwrap_or(inner);
        let mut items = Vec::new();
        let (mut depth, mut quote, mut start) = (0i32, false, 0);
        for (i, c) in inner.char_indices() {
            match c {
                '"' => quote = !quote,
                '[' | '{' if !quote => depth += 1,
                ']' | '}' if !quote => depth -= 1,
                ',' if !quote && depth == 0 => {
                    items.push(&inner[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        items.push(&inner[start..]);
        items
            .into_iter()
            .map(|item| unquote(item.trim()).to_owned())
            .filter(|item| !item.is_empty())
            .collect()
    }
    pub fn hardware_metrics_interval(&self) -> u64 {
        self.integer("hardware_metrics_interval", 1000)
            .clamp(500, 10000) as u64
    }
    /// `virtual_display_mode`; unset, it is per client where the OS ships the
    /// IddCx the driver needs (`os_gate::virtual_display`, Windows 11 23H2 and
    /// later) and disabled before that, as Vibepollo does on Windows 10.
    pub fn virtual_display_mode(&self, windows_11: bool) -> &str {
        self.get(
            "virtual_display_mode",
            if windows_11 { "per_client" } else { "disabled" },
        )
    }
    pub fn log_level(&self) -> &'static str {
        match self
            .get("min_log_level", "info")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "0" | "verbose" | "trace" => "trace",
            "1" | "debug" => "debug",
            "2" | "info" => "info",
            "3" | "warning" | "warn" => "warn",
            "4" | "5" | "error" | "fatal" => "error",
            "6" | "none" | "off" => "off",
            _ => "info",
        }
    }
    /// The base port. An out-of-range port is ignored, as in Vibepollo.
    pub fn port(&self) -> Result<u16> {
        let n = self.integer("port", 47989);
        if !PORTS.contains(&n) {
            invalid("port", self.get("port", ""));
            return Ok(47989);
        }
        Ok(n as u16)
    }
    pub fn ports(&self) -> Result<Ports> {
        Ok(Ports::from_base(self.port()?))
    }
    /// Client roaming (`client_roaming`, on by default): DESCRIBE offers it, and a client
    /// that proves a new address keeps its stream. Nothing happens until an authenticated
    /// probe arrives (`roaming`).
    pub fn client_roaming(&self) -> bool {
        self.boolean("client_roaming", true)
    }
    /// HDR10+ and HDR Vivid dynamic metadata in HDR streams (`hdr_dynamic_metadata`, on
    /// by default): per-frame brightness statistics for displays that tone-map with them.
    pub fn hdr_dynamic_metadata(&self) -> bool {
        self.boolean("hdr_dynamic_metadata", true)
    }
    /// PCM, Dolby Digital (AC3) and Dolby Digital Plus (E-AC3) audio for clients that pick one
    /// (`audio_passthrough`, on by default): serverinfo and DESCRIBE offer the codecs, and stock
    /// clients keep Opus. AC3 and E-AC3 also need FFmpeg's libraries beside the host.
    pub fn audio_passthrough(&self) -> bool {
        self.boolean("audio_passthrough", true)
    }
    /// The most video FEC a device may ask for (`rlFecPercentage`) or reach
    /// through its loss reports, in percent of the video packets
    /// (`fec_max_percentage`, 0 to 255, 50 by default).
    pub fn fec_max_percentage(&self) -> u8 {
        let n = self.integer("fec_max_percentage", 50);
        if !(0..=255).contains(&n) {
            invalid("fec_max_percentage", self.get("fec_max_percentage", ""));
            return 50;
        }
        n as u8
    }
    /// `GET /bandwidth-probe` for paired devices that time a download before
    /// they pick a bitrate (`bandwidth_probe`, on by default).
    pub fn bandwidth_probe(&self) -> bool {
        self.boolean("bandwidth_probe", true)
    }
    /// Show the device's keyboard when a text field on the host gets the focus
    /// (`keyboard_follows_focus`, on by default, Experimental): the host offers
    /// text input (0x553B) and watches the focus for devices that ask.
    pub fn keyboard_follows_focus(&self) -> bool {
        self.boolean("keyboard_follows_focus", true)
    }
    /// Send the text around the cursor to the device's keyboard
    /// (`text_input_surrounding_text`, off by default); never for password or
    /// other sensitive fields.
    pub fn text_input_surrounding_text(&self) -> bool {
        self.boolean("text_input_surrounding_text", false)
    }
    /// `keyboard_layout`: `client` (the default, the layout the device reports
    /// in 0x553E), `us` or an 8-hex-digit Windows layout id such as `00000407`.
    /// The host turns a device's keys into scan codes with it.
    pub fn keyboard_layout(&self) -> KeyboardLayoutSetting {
        match unquote(self.get("keyboard_layout", "client").trim()).trim() {
            "" | "client" => KeyboardLayoutSetting::Client,
            "us" => KeyboardLayoutSetting::Us,
            other => match parse_klid(other) {
                Some(klid) => KeyboardLayoutSetting::Klid(klid),
                None => {
                    invalid("keyboard_layout", other);
                    KeyboardLayoutSetting::Client
                }
            },
        }
    }
    /// The WebRTC server for TV and browser clients (off by default).
    pub fn webrtc_server(&self) -> bool {
        self.boolean("webrtc_server", false)
    }
    /// File transfer from paired devices (off by default). A device also
    /// needs its Upload files or Download files permission.
    pub fn file_transfer(&self) -> bool {
        self.boolean("file_transfer", false)
    }
    /// USB forwarding from devices with `allow_usb` (off by default,
    /// Experimental).
    pub fn usb_forwarding(&self) -> bool {
        self.boolean("usb_forwarding", false)
    }
    /// How many forwarded USB devices the host takes at once
    /// (`usb_max_devices`, 1 to 16, 4 by default). Outside that is reported
    /// and the default is used.
    pub fn usb_max_devices(&self) -> u8 {
        let n = self.integer("usb_max_devices", 4);
        if (1..=16).contains(&n) {
            n as u8
        } else {
            invalid("usb_max_devices", self.get("usb_max_devices", ""));
            4
        }
    }
    /// The host-drawn couch menu (on by default, Experimental). Off means no
    /// input filter, no menu worker and no `session.couch_menu` feature.
    pub fn couch_menu(&self) -> bool {
        self.boolean("couch_menu", true)
    }
    /// The folder transfers use; `None` (empty) is the signed-in user's
    /// `Downloads\Rubylight`.
    pub fn file_transfer_dir(&self) -> Option<std::path::PathBuf> {
        let dir = unquote(self.get("file_transfer_dir", "").trim()).trim();
        (!dir.is_empty()).then(|| std::path::PathBuf::from(dir))
    }
    /// The largest upload in bytes (`file_transfer_max_mb`, MiB, 4096 by
    /// default). Below 1 MiB is reported and the default is used.
    pub fn file_transfer_max_bytes(&self) -> u64 {
        const MIB: i64 = 1024 * 1024;
        let mut mib = self.integer("file_transfer_max_mb", 4096);
        if !(1..=i64::MAX / MIB).contains(&mib) {
            invalid("file_transfer_max_mb", self.get("file_transfer_max_mb", ""));
            mib = 4096;
        }
        (mib * MIB) as u64
    }
    /// How fast uploads are written while a stream runs, in bytes per
    /// second (`file_transfer_stream_mbps`, 50 by default). 0 or less does
    /// not slow them.
    pub fn file_transfer_stream_rate(&self) -> Option<u64> {
        let mbps = self.integer("file_transfer_stream_mbps", 50);
        (mbps > 0).then(|| (mbps as u64).saturating_mul(125_000))
    }
    /// Whether the WebRTC media thread paces video (the default). `webrtc_pacing = false` sends
    /// every packet the moment str0m makes it; it exists to measure the two against each other.
    pub fn webrtc_pacing(&self) -> bool {
        self.boolean("webrtc_pacing", true)
    }
    /// Whether the WebRTC offer carries NACK and RTX, so a lost video packet is resent
    /// (`webrtc_rtx = false` turns it off). On by default: at 0.1 % datagram loss it kept 60 fps
    /// with no freeze where keyframe recovery alone dropped to 36 to 43 fps with 3 to 18 freezes,
    /// and on a clean link it changes nothing (rust/PERFORMANCE.md).
    pub fn webrtc_rtx(&self) -> bool {
        self.boolean("webrtc_rtx", true)
    }
    /// Video datagrams the WebRTC media thread drops per thousand (`webrtc_loss_permille`, 0 to
    /// 1000), as a lossy link does. A measurement switch, 0 by default.
    pub fn webrtc_loss_permille(&self) -> u32 {
        self.integer("webrtc_loss_permille", 0).clamp(0, 1000) as u32
    }
    /// The WebRTC signalling port (TCP; discovery always listens on UDP 8000).
    /// A value outside 1024..=65535 is reported and the default is used.
    pub fn webrtc_port(&self) -> u16 {
        self.webrtc_port_setting("webrtc_port", WEBRTC_PORT)
    }
    /// The WebRTC media port (UDP).
    pub fn webrtc_media_port(&self) -> u16 {
        self.webrtc_port_setting("webrtc_media_port", WEBRTC_MEDIA_PORT)
    }
    fn webrtc_port_setting(&self, key: &str, default: u16) -> u16 {
        let n = self.integer(key, i64::from(default));
        if !WEBRTC_PORTS.contains(&n) {
            invalid(key, self.get(key, ""));
            return default;
        }
        n as u16
    }
    /// Before 2.0.0 `amd_ltr_frames` defaulted to 0, and a console save
    /// could write that default into sunshine.conf, which kept AV1's
    /// recovery without a keyframe off after the upgrade. Until `upgraded`,
    /// a saved 0 is taken for that old default and dropped, so the current
    /// one applies; a 0 saved after the upgrade stays. Returns whether the
    /// saved value was dropped.
    pub fn upgrade_ltr_default(&mut self, upgraded: bool) -> bool {
        let old_default = !upgraded
            && self
                .values
                .get("amd_ltr_frames")
                .is_some_and(|value| parse_integer(value) == Some(0));
        if old_default {
            self.values.remove("amd_ltr_frames");
        }
        old_default
    }
    pub fn text(&self) -> String {
        self.values
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.values
                .iter()
                .map(|(k, v)| {
                    let parsed = serde_json::from_str(v)
                        .unwrap_or_else(|_| serde_json::Value::String(v.clone()));
                    (k.clone(), parsed)
                })
                .collect(),
        )
    }
    pub fn update(&mut self, object: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
        // A setting sent under an earlier name is saved under its current
        // one, unless that is sent too.
        let mut object = object.clone();
        for &(old, new) in RENAMED_KEYS {
            if let Some(value) = object.remove(old)
                && !object.contains_key(new)
            {
                object.insert(new.to_owned(), value);
            }
        }
        object.retain(|key, _| !removed(key));
        let object = &object;
        let mut next = self.clone();
        for (key, value) in object {
            if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || key.is_empty() {
                bail!("invalid configuration key");
            }
            // Legacy PATCH and POST both reset a setting when its value is
            // null/empty. Keeping the literal "null" silently defeats defaults.
            if value.is_null() || value.as_str() == Some("") {
                next.values.remove(key);
                continue;
            }
            let value = match value {
                serde_json::Value::String(v) => v.clone(),
                _ => value.to_string(),
            };
            if value.contains('\0') {
                bail!("configuration cannot contain NUL");
            }
            if value.contains(['\r', '\n'])
                && !(value.trim().starts_with('[') || value.trim().starts_with('{'))
            {
                bail!("configuration values cannot inject new keys");
            }
            // A client or an old console sending a retired value saves its
            // replacement, as the file would be read.
            let value = replacement(key, &value).map_or(value, str::to_owned);
            next.values.insert(key.clone(), value);
        }
        // Only what changes is checked: a value the file already had (the
        // host reads it leniently) must not block every other save.
        let changed = |key: &str| object.contains_key(key) && next.values.contains_key(key);
        if changed("port") && !PORTS.contains(&next.integer("port", 0)) {
            bail!("port must be between 1029 and 65514");
        }
        for key in ["webrtc_port", "webrtc_media_port"] {
            if changed(key) && !WEBRTC_PORTS.contains(&next.integer(key, 0)) {
                bail!("{key} must be between 1024 and 65535");
            }
        }
        if changed("frame_limiter_fps_limit") {
            crate::framegen::Rate::parse(next.get("frame_limiter_fps_limit", "0"))?;
        }
        if changed(crate::stream_preset::SETTING)
            && !crate::stream_preset::valid(next.get(crate::stream_preset::SETTING, ""))
        {
            bail!("stream_preset must be lowest_latency, balanced or reference");
        }
        if changed("hardware_metrics_interval")
            && next.values.contains_key("hardware_metrics_interval")
            && !(500..=10000).contains(&next.integer("hardware_metrics_interval", 0))
        {
            bail!("hardware metrics interval must be between 500 and 10000 milliseconds");
        }
        for key in [
            "audio_night_mode",
            "audio_headphone_surround",
            "sleep_while_suspended",
        ] {
            if changed(key) && parse_boolean(&next.values[key]).is_none() {
                bail!("{key} must be true or false");
            }
        }
        if changed("sleep_after_idle_minutes")
            && !parse_integer(&next.values["sleep_after_idle_minutes"])
                .is_some_and(|minutes| minutes == 0 || (5..=1440).contains(&minutes))
        {
            bail!("sleep_after_idle_minutes must be 0 (off) or between 5 and 1440");
        }
        for key in crate::audio_haptics::SETTINGS {
            if changed(key) {
                crate::audio_haptics::check(key, &next.values[key])?;
            }
        }
        for key in crate::couch_menu::chord::SETTINGS {
            if changed(key)
                && let Some(warning) = crate::couch_menu::chord::check(key, &next.values[key])
                    .map_err(anyhow::Error::msg)?
            {
                tracing::warn!("{warning}");
            }
        }
        // Warn, not refuse: either setting alone is fine. The couch menu
        // values were checked above, so the read refuses none.
        if crate::couch_menu::chord::SETTINGS
            .into_iter()
            .chain(["back_button_timeout"])
            .any(changed)
            && let Some(warning) =
                crate::couch_menu::chord::OpenWith::read(&next, &mut |_, _, _| {})
                    .select_conflict(crate::input_policy::back_button_timeout(&next))
        {
            tracing::warn!("{warning}");
        }
        if changed("hardware_metrics_selection") {
            for metric in next.list("hardware_metrics_selection") {
                if !HARDWARE_METRICS.contains(&metric.as_str()) {
                    bail!("unknown hardware metric: {metric}");
                }
            }
        }
        *self = next;
        Ok(())
    }
    /// Before `keys` are written to sunshine.conf: values the file would not
    /// give back, and settings that would keep the host from starting.
    pub fn check_saved<'a>(&self, keys: impl IntoIterator<Item = &'a String>) -> Result<()> {
        for key in keys {
            let key = current_name(key);
            let Some(value) = self.values.get(key) else {
                continue;
            };
            // '#' starts a comment and a value opening [ or { runs until it
            // closes, so such a value would be cut short, or swallow the
            // settings after it, on the next start.
            let expected = value.lines().map(str::trim).collect::<Vec<_>>().join("\n");
            let stored = Self::parse(&format!("{key} = {value}\n"))?;
            if stored.values.get(key).map(String::as_str) != Some(expected.trim()) {
                bail!(
                    "{key} cannot be saved: in the configuration file # starts a comment and a value opening [ or {{ has to close it"
                );
            }
            let value = unquote(value.trim());
            if key == "bind_address" && value.parse::<std::net::IpAddr>().is_err() {
                bail!("bind_address must be an IP address of this PC, or blank for all of them");
            }
            if key == "log_path"
                && (value.ends_with(['\\', '/'])
                    || (Path::new(value).is_absolute() && Path::new(value).is_dir()))
            {
                bail!("log_path must name a file, not a folder");
            }
        }
        Ok(())
    }
    pub fn path(&self, key: &str, directory: &Path, default: &str) -> PathBuf {
        let p = PathBuf::from(self.get(key, default));
        if p.is_absolute() {
            p
        } else {
            directory.join(p)
        }
    }
    pub fn display_request(&self, width: u32, height: u32, fps: u32) -> Result<DisplayRequest> {
        let mut request = self.display_request_rate(
            width,
            height,
            crate::framegen::Rate(fps.saturating_mul(1000)),
            false,
            false,
        )?;
        request.refresh = request.refresh.map(|r| r.saturating_add(500) / 1000);
        Ok(request)
    }
    pub fn display_request_rate(
        &self,
        width: u32,
        height: u32,
        rate: crate::framegen::Rate,
        hdr: bool,
        virtual_display: bool,
    ) -> Result<DisplayRequest> {
        if !virtual_display && self.get("dd_configuration_option", "verify_only") == "disabled" {
            return Ok(DisplayRequest {
                resolution: None,
                refresh: None,
                prefer_highest: false,
                hdr: None,
            });
        }
        let resolution_option = self.get("dd_resolution_option", "auto");
        let refresh_option = self.get("dd_refresh_rate_option", "auto");
        let resolution_option = match resolution_option {
            "disabled" | "auto" | "manual" => resolution_option,
            other => {
                fallback("dd_resolution_option", other, "auto");
                "auto"
            }
        };
        let refresh_option = match refresh_option {
            "disabled" | "auto" | "manual" | "prefer_highest" => refresh_option,
            other => {
                fallback("dd_refresh_rate_option", other, "auto");
                "auto"
            }
        };
        let mut resolution = match resolution_option {
            "disabled" => None,
            "auto" => Some((width, height)),
            "manual" => {
                let (width, height) = self
                    .get("dd_manual_resolution", "")
                    .split_once('x')
                    .context("manual resolution must be WIDTHxHEIGHT")?;
                Some((width.trim().parse()?, height.trim().parse()?))
            }
            _ => bail!("invalid display resolution policy"),
        };
        let mut refresh = match refresh_option {
            "disabled" => None,
            "auto" => Some(rate.0),
            "manual" => {
                let rate = crate::framegen::Rate::parse(self.get("dd_manual_refresh_rate", ""))?;
                if rate.0 < 1000 {
                    bail!("invalid manual refresh rate");
                }
                Some(rate.0)
            }
            "prefer_highest" => None,
            _ => bail!("invalid display refresh policy"),
        };
        let remapping_type = match (resolution_option == "auto", refresh_option == "auto") {
            (true, true) => Some("mixed"),
            (true, false) => Some("resolution_only"),
            (false, true) => Some("refresh_rate_only"),
            _ => None,
        };
        if let Some(kind) = remapping_type
            && let Some(value) = self.values.get("dd_mode_remapping")
        {
            let mappings: serde_json::Value =
                serde_json::from_str(value).context("invalid display mode remapping")?;
            if let Some(entries) = mappings.get(kind).and_then(serde_json::Value::as_array) {
                for entry in entries {
                    let text = |key: &str| {
                        entry
                            .get(key)
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .trim()
                    };
                    let parse_resolution = |s: &str| -> Result<(u32, u32)> {
                        let (w, h) = s
                            .split_once('x')
                            .context("remapping resolution must be WIDTHxHEIGHT")?;
                        Ok((w.trim().parse()?, h.trim().parse()?))
                    };
                    if kind != "refresh_rate_only"
                        && !text("requested_resolution").is_empty()
                        && resolution != Some(parse_resolution(text("requested_resolution"))?)
                    {
                        continue;
                    }
                    if kind != "resolution_only"
                        && !text("requested_fps").is_empty()
                        && refresh != Some(crate::framegen::Rate::parse(text("requested_fps"))?.0)
                    {
                        continue;
                    }
                    if kind != "refresh_rate_only" && !text("final_resolution").is_empty() {
                        resolution = Some(parse_resolution(text("final_resolution"))?);
                    }
                    if kind != "resolution_only" && !text("final_refresh_rate").is_empty() {
                        refresh = Some(crate::framegen::Rate::parse(text("final_refresh_rate"))?.0);
                    }
                    break;
                }
            }
        }
        if resolution.is_some_and(|(w, h)| !(320..=7680).contains(&w) || !(200..=4320).contains(&h))
            || refresh.is_some_and(|f| !(1000..=1_000_000).contains(&f))
        {
            bail!("display mode is outside its limits");
        }
        Ok(DisplayRequest {
            resolution,
            refresh,
            prefer_highest: refresh_option == "prefer_highest",
            hdr: if self.get("dd_hdr_option", "auto") == "disabled" {
                None
            } else {
                Some(match self.get("dd_hdr_request_override", "auto") {
                    "force_on" => true,
                    "force_off" => false,
                    other => {
                        fallback("dd_hdr_request_override", other, "auto");
                        hdr
                    }
                })
            },
        })
    }
}
/// Base ports whose derived ports (base - 5 to base + 21) are all valid.
const PORTS: std::ops::RangeInclusive<i64> = 1029..=65514;
/// The WebRTC ports the TV expects by default, and the range a saved port may take.
pub const WEBRTC_PORT: u16 = 8000;
pub const WEBRTC_MEDIA_PORT: u16 = 8001;
const WEBRTC_PORTS: std::ops::RangeInclusive<i64> = 1024..=65535;
/// `keyboard_layout`: the layout the host types a device's keys with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardLayoutSetting {
    /// The layout the device reports (0x553E).
    Client,
    /// US English.
    Us,
    /// A Windows keyboard layout id (KLID), written as 8 hex digits.
    Klid(u32),
}

/// A KLID as Windows writes it: exactly 8 hex digits, such as `00000407`.
pub fn parse_klid(text: &str) -> Option<u32> {
    (text.len() == 8 && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(text, 16).ok())
        .flatten()
}

/// Whether a device or an app may override `key` for its streams. As in
/// Vibepollo, overrides cover stream, input, display and encoder behaviour;
/// identity, network, paths and integrations stay host-wide.
pub fn override_allowed(key: &str) -> bool {
    const ALLOWED: &[&str] = &[
        // Input
        "controller",
        "gamepad",
        "ds4_back_as_touchpad_click",
        "motion_as_ds4",
        "touchpad_as_ds4",
        "back_button_timeout",
        "back_grip_r4",
        "back_grip_l4",
        "back_grip_r5",
        "back_grip_l5",
        "steam_deck_controller",
        "keyboard",
        "key_repeat_delay",
        "key_repeat_frequency",
        "always_send_scancodes",
        "key_rightalt_to_key_win",
        "mouse",
        "high_resolution_scrolling",
        "native_pen_touch",
        "keybindings",
        "couch_menu",
        "couch_menu_open_with",
        "couch_menu_controller_chord",
        "couch_menu_hold_ms",
        "couch_menu_corner_hold_ms",
        "couch_menu_keyboard_chord",
        "audio_haptics",
        "audio_haptics_strength",
        "audio_haptics_curve",
        "audio_haptics_gate",
        // Audio, video and display automation
        "audio_sink",
        "audio_capture_mode",
        "audio_sink_capture_only",
        "audio_night_mode",
        "audio_headphone_surround",
        "virtual_sink",
        "stream_audio",
        "stream_mic",
        "adapter_name",
        "adapter_pnp_id",
        "dd_configuration_option",
        "dd_resolution_option",
        "dd_manual_resolution",
        "dd_refresh_rate_option",
        "dd_manual_refresh_rate",
        "dd_hdr_option",
        "dd_hdr_request_override",
        "dd_config_revert_delay",
        "dd_config_revert_on_disconnect",
        "dd_paused_virtual_display_timeout_secs",
        "dd_always_restore_from_golden",
        "dd_snapshot_exclude_devices",
        "dd_snapshot_restore_hotkey",
        "dd_snapshot_restore_hotkey_modifiers",
        "dd_activate_virtual_display",
        "dd_virtual_display_scale",
        "dd_virtual_display_permanent_count",
        "dd_mode_remapping",
        "max_bitrate",
        "minimum_fps_target",
        "stream_reconfigure",
        "stream_preset",
        // Codec and capture
        "fec_percentage",
        "adaptive_fec",
        "host_adaptive_bitrate",
        "host_adaptive_pacing",
        "wireless_adaptive_pacing",
        "keyframe_loss_pacing",
        "link_rate_adaptation",
        "video_max_batch_size_kb",
        "pyrowave_critical_fec_percentage",
        "hevc_mode",
        "av1_mode",
        "experimental_packed_444",
        "capture",
        "encoder",
        // Playnite focus
        "playnite_focus_attempts",
        "playnite_focus_timeout_secs",
        "playnite_focus_exit_on_first",
        // Frame limiter
        "frame_limiter_enable",
        "frame_limiter_provider",
        "frame_limiter_fps_limit",
        "frame_limiter_auto_virtual_framegen",
        "rtss_frame_limit_type",
        // Encoder tuning
        "amd_usage",
        "amd_rc",
        "amd_peak_bitrate_ratio",
        "amd_vbv_buffer_frames",
        "amd_max_frame_size",
        "amd_qvbr_quality_level",
        "amd_enforce_hrd",
        "amd_quality",
        "amd_preanalysis",
        "amd_vbaq",
        "amd_coder",
        "amd_ltr_frames",
        "amd_input_queue_size",
        "amd_smart_access_video",
        "amd_split_frame",
        "amd_lowlatency_mode",
        "amd_high_motion_quality_boost",
        "amd_av1_screen_content",
        "amd_av1_latency_mode",
        "encoder_focus",
        "hdr_peak_brightness",
        // 3.0 extensions
        "hdr_dynamic_metadata",
        "hdr_dolby_vision",
        "hdr_hlg",
        "fec_max_percentage",
        "audio_passthrough",
        "audio_surround_714",
        "keyboard_follows_focus",
        "text_input_surrounding_text",
        "keyboard_layout",
        "touch_mode",
        "touch_sensitivity",
        "touch_gestures",
        "client_cursor",
        "dd_rotation_option",
        "dd_manual_rotation",
        "dd_scale_option",
        "dd_manual_scale",
    ];
    ALLOWED.contains(&key)
}
/// Report a setting value the host does not understand and will not use.
/// Each key and value is reported once.
pub fn invalid(key: &str, value: &str) {
    if first_report(key, value) {
        tracing::warn!(key, value, "unsupported setting value; using the default");
    }
}
/// [`invalid`], with the parse error that refused the value.
pub fn refused(key: &str, value: &str, error: &str) {
    if first_report(key, value) {
        tracing::warn!(
            key,
            value,
            error,
            "unsupported setting value; using the default"
        );
    }
}
fn first_report(key: &str, value: &str) -> bool {
    static SEEN: std::sync::Mutex<std::collections::BTreeSet<(String, String)>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    SEEN.lock()
        .unwrap()
        .insert((key.to_owned(), value.to_owned()))
}
/// Report `value` unless it is the default the caller falls back to.
pub fn fallback(key: &str, value: &str, default: &str) {
    if value != default {
        invalid(key, value);
    }
}
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}
/// A number setting's text as the host parses it: Vibepollo also writes
/// numbers in quotes (`"30"`), and the console shows them as numbers.
pub fn number_text(value: &str) -> &str {
    unquote(value.trim()).trim()
}
/// A boolean setting's text as the host reads it, `None` when it is neither.
pub fn parse_boolean(value: &str) -> Option<bool> {
    match unquote(value.trim()).to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" | "enable" | "enabled" | "on" => Some(true),
        "false" | "no" | "0" | "disable" | "disabled" | "off" => Some(false),
        _ => None,
    }
}
pub fn parse_integer(value: &str) -> Option<i64> {
    let value = number_text(value);
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return i64::from_str_radix(hex, 16).ok();
    }
    value.parse().ok()
}
/// A shared folder's id, as `shared_folders` defines it and a device grant
/// names it: 1 to 64 ASCII letters, digits, `-` or `_`, so that it never
/// reads as a path.
pub fn valid_folder_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
/// The text of sunshine.conf. Editors save it as UTF-8 with or without a
/// byte order mark, or as UTF-16 with one; anything else is read as the
/// Windows ANSI code page (Windows-1252) rather than refused.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| unit(pair))
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return utf16(rest, u16::from_be_bytes);
    }
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    tracing::warn!("the configuration is not UTF-8; it is read as Windows-1252");
    // Windows-1252 differs from Latin-1 only in 0x80-0x9F; its five unused
    // codes map to the same control characters, as browsers read them.
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}',
        '\u{8f}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}',
        '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}',
        '\u{178}',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9f => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}
/// The line without its comment. Quotes protect `#` only when they are
/// balanced on the line; otherwise `#` always starts a comment, as in Vibepollo.
fn strip_comment(line: &str) -> String {
    let mut quote = false;
    let mut escaped = false;
    let mut cut = None;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quote => escaped = true,
            '"' => quote = !quote,
            '#' if !quote && cut.is_none() => cut = Some(i),
            _ => {}
        }
    }
    let cut = if quote { line.find('#') } else { cut };
    line[..cut.unwrap_or(line.len())].to_owned()
}
/// Net bracket depth of a line, ignoring brackets in balanced quotes.
fn bracket_depth(line: &str) -> i32 {
    let balanced = line.matches('"').count().is_multiple_of(2);
    let (mut depth, mut quote, mut escaped) = (0, false, false);
    for c in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quote => escaped = true,
            '"' if balanced => quote = !quote,
            '[' | '{' if !quote => depth += 1,
            ']' | '}' if !quote => depth -= 1,
            _ => {}
        }
    }
    depth
}
pub struct DisplayRequest {
    pub resolution: Option<(u32, u32)>,
    pub refresh: Option<u32>,
    pub prefer_highest: bool,
    pub hdr: Option<bool>,
}
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Ports {
    pub http: u16,
    pub https: u16,
    pub web: u16,
    pub video: u16,
    pub control: u16,
    pub audio: u16,
    pub mic: u16,
    pub rtsp: u16,
}
impl Ports {
    pub fn from_base(n: u16) -> Self {
        Self {
            http: n,
            https: n - 5,
            web: n + 1,
            video: n + 9,
            control: n + 10,
            audio: n + 11,
            mic: n + crate::mic::PORT_OFFSET,
            rtsp: n + 21,
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn folder_ids_are_short_plain_names() {
        for id in ["games", "saves-2", "a_b", &"x".repeat(64)] {
            assert!(super::valid_folder_id(id), "{id}");
        }
        for id in ["", "../games", "a/b", "a b", "ü", &"x".repeat(65)] {
            assert!(!super::valid_folder_id(id), "{id}");
        }
    }

    #[test]
    fn webrtc_video_is_paced_unless_the_measurement_switch_turns_it_off() {
        let mut config = super::Config::default();
        assert!(config.webrtc_pacing());
        let saved = serde_json::json!({"webrtc_pacing": "false"});
        config.update(saved.as_object().unwrap()).unwrap();
        assert!(!config.webrtc_pacing());
    }

    #[test]
    fn file_transfer_is_off_with_4_gib_uploads_paced_at_50_mbps_while_streaming() {
        let mut config = super::Config::default();
        assert!(!config.file_transfer());
        assert_eq!(config.file_transfer_dir(), None);
        assert_eq!(config.file_transfer_max_bytes(), 4096 << 20);
        assert_eq!(config.file_transfer_stream_rate(), Some(6_250_000));
        let saved = serde_json::json!({
            "file_transfer": true,
            "file_transfer_dir": "\"D:\\Inbox\"",
            "file_transfer_max_mb": "2",
            "file_transfer_stream_mbps": 0,
        });
        config.update(saved.as_object().unwrap()).unwrap();
        assert!(config.file_transfer());
        assert_eq!(
            config.file_transfer_dir(),
            Some(std::path::PathBuf::from("D:\\Inbox"))
        );
        assert_eq!(config.file_transfer_max_bytes(), 2 << 20);
        assert_eq!(config.file_transfer_stream_rate(), None);
        for bad in ["0", "-1", "9223372036854775807"] {
            let saved =
                serde_json::json!({ "file_transfer_max_mb": bad, "file_transfer_dir": " " });
            config.update(saved.as_object().unwrap()).unwrap();
            assert_eq!(config.file_transfer_max_bytes(), 4096 << 20, "{bad}");
            assert_eq!(config.file_transfer_dir(), None);
        }
    }

    #[test]
    fn usb_forwarding_is_off_with_4_devices_at_once() {
        let mut config = super::Config::default();
        assert!(!config.usb_forwarding());
        assert_eq!(config.usb_max_devices(), 4);
        let saved = serde_json::json!({"usb_forwarding": "enabled", "usb_max_devices": "16"});
        config.update(saved.as_object().unwrap()).unwrap();
        assert!(config.usb_forwarding());
        assert_eq!(config.usb_max_devices(), 16);
        for bad in ["0", "17", "-1", "lots"] {
            let saved = serde_json::json!({ "usb_max_devices": bad });
            config.update(saved.as_object().unwrap()).unwrap();
            assert_eq!(config.usb_max_devices(), 4, "{bad}");
        }
        let saved = serde_json::json!({"usb_max_devices": 1});
        config.update(saved.as_object().unwrap()).unwrap();
        assert_eq!(config.usb_max_devices(), 1);
    }

    #[test]
    fn webrtc_rtx_is_on_and_datagram_loss_is_off_until_set() {
        let mut config = super::Config::default();
        assert!(config.webrtc_rtx());
        assert_eq!(config.webrtc_loss_permille(), 0);
        let saved = serde_json::json!({"webrtc_rtx": "false", "webrtc_loss_permille": "1500"});
        config.update(saved.as_object().unwrap()).unwrap();
        assert!(!config.webrtc_rtx());
        assert_eq!(config.webrtc_loss_permille(), 1000);
    }

    #[test]
    fn audio_passthrough_is_on_unless_turned_off() {
        assert!(super::Config::default().audio_passthrough());
        for (text, on) in [
            ("audio_passthrough = true\n", true),
            ("audio_passthrough = disabled\n", false),
            ("audio_passthrough = 0\n", false),
        ] {
            assert_eq!(
                super::Config::parse(text).unwrap().audio_passthrough(),
                on,
                "{text}"
            );
        }
    }

    #[test]
    fn client_roaming_is_on_unless_turned_off() {
        assert!(super::Config::default().client_roaming());
        for (text, on) in [
            ("client_roaming = true\n", true),
            ("client_roaming = off\n", false),
            ("client_roaming = 0\n", false),
        ] {
            assert_eq!(
                super::Config::parse(text).unwrap().client_roaming(),
                on,
                "{text}"
            );
        }
    }

    #[test]
    fn hdr_dynamic_metadata_is_on_unless_turned_off() {
        assert!(super::Config::default().hdr_dynamic_metadata());
        for (text, on) in [
            ("hdr_dynamic_metadata = true\n", true),
            ("hdr_dynamic_metadata = disabled\n", false),
            ("hdr_dynamic_metadata = 0\n", false),
        ] {
            assert_eq!(
                super::Config::parse(text).unwrap().hdr_dynamic_metadata(),
                on,
                "{text}"
            );
        }
    }

    #[test]
    fn the_fec_cap_is_0_to_255_percent_and_50_unless_set() {
        assert_eq!(super::Config::default().fec_max_percentage(), 50);
        for (text, cap) in [
            ("fec_max_percentage = 0\n", 0),
            ("fec_max_percentage = 255\n", 255),
            ("fec_max_percentage = 0x1e\n", 30),
            ("fec_max_percentage = -1\n", 50),
            ("fec_max_percentage = 256\n", 50),
            ("fec_max_percentage = lots\n", 50),
        ] {
            assert_eq!(
                super::Config::parse(text).unwrap().fec_max_percentage(),
                cap,
                "{text}"
            );
        }
    }

    #[test]
    fn the_keyboard_follows_text_fields_without_their_text_by_default() {
        let c = super::Config::default();
        assert!(c.keyboard_follows_focus() && !c.text_input_surrounding_text());
        let c = super::Config::parse(
            "keyboard_follows_focus = disabled\ntext_input_surrounding_text = enabled\n",
        )
        .unwrap();
        assert!(!c.keyboard_follows_focus() && c.text_input_surrounding_text());
        for key in [
            "keyboard_follows_focus",
            "text_input_surrounding_text",
            "keyboard_layout",
        ] {
            assert!(super::override_allowed(key), "{key}");
        }
    }

    #[test]
    fn the_bandwidth_probe_is_on_unless_turned_off() {
        assert!(super::Config::default().bandwidth_probe());
        for (text, on) in [
            ("bandwidth_probe = true\n", true),
            ("bandwidth_probe = disabled\n", false),
            ("bandwidth_probe = 0\n", false),
        ] {
            assert_eq!(
                super::Config::parse(text).unwrap().bandwidth_probe(),
                on,
                "{text}"
            );
        }
    }

    #[test]
    fn keyboard_layout_takes_client_us_or_a_klid() {
        use super::KeyboardLayoutSetting::{Client, Klid, Us};
        let layout = |text: &str| super::Config::parse(text).unwrap().keyboard_layout();
        assert_eq!(super::Config::default().keyboard_layout(), Client);
        for (text, expected) in [
            ("keyboard_layout = client", Client),
            ("keyboard_layout = us", Us),
            ("keyboard_layout = 00000407", Klid(0x407)),
            ("keyboard_layout = \"0001041f\"", Klid(0x1041F)),
            // What the host can't use is reported and the device's layout applies.
            ("keyboard_layout = de", Client),
            ("keyboard_layout = 0000040", Client),
            ("keyboard_layout = 0000040G", Client),
            ("keyboard_layout = 000004070", Client),
        ] {
            assert_eq!(layout(text), expected, "{text}");
        }
    }

    #[test]
    fn the_webrtc_server_is_off_on_8000_and_8001_and_refuses_bad_ports() {
        let config = super::Config::default();
        assert!(!config.webrtc_server());
        assert_eq!(
            (config.webrtc_port(), config.webrtc_media_port()),
            (8000, 8001)
        );
        let mut config = super::Config::default();
        let saved = serde_json::json!({"webrtc_server": true, "webrtc_port": 9000, "webrtc_media_port": "9001"});
        config.update(saved.as_object().unwrap()).unwrap();
        assert!(config.webrtc_server());
        assert_eq!(
            (config.webrtc_port(), config.webrtc_media_port()),
            (9000, 9001)
        );
        for bad in [
            serde_json::json!(80),
            serde_json::json!(70000),
            serde_json::json!("x"),
        ] {
            for key in ["webrtc_port", "webrtc_media_port"] {
                let saved = serde_json::json!({ key: bad });
                assert!(
                    config.update(saved.as_object().unwrap()).is_err(),
                    "{key} {bad}"
                );
            }
        }
        // A hand-edited file with a bad port falls back loudly to the default.
        let config = super::Config::parse(
            "webrtc_port = 80
webrtc_media_port = 99999
",
        )
        .unwrap();
        assert_eq!(
            (config.webrtc_port(), config.webrtc_media_port()),
            (8000, 8001)
        );
    }
    #[test]
    fn direct_driver_push_defaults_to_the_fence_ring_unless_turned_off() {
        let fence = |text: &str| super::Config::parse(text).unwrap().idd_push_fence_ring();
        assert!(fence(""));
        assert!(fence("port = 47989"));
        assert!(fence("idd_push_fence_ring = true"));
        assert!(!fence("idd_push_fence_ring = false"));
        assert!(!fence("idd_push_fence_ring = off"));
        assert!(super::Config::default().idd_push_fence_ring());
    }
    #[test]
    fn removed_rtx_hdr_settings_go_quietly_and_its_peak_brightness_is_renamed() {
        // Profiles, apps and devices saved before RTX HDR was removed.
        let mut config = super::Config::parse(
            "rtx_hdr = true\nrtx_hdr_contrast = 14\nrtx_hdr_sdr_brightness = 20\nrtx_hdr_peak_brightness = 1500\nport = 47989\n",
        )
        .unwrap();
        for key in super::REMOVED_KEYS {
            assert!(!config.values.contains_key(*key), "{key}");
        }
        assert_eq!(config.integer("hdr_peak_brightness", 0), 1500);
        assert_eq!(config.integer("port", 0), 47989);
        // A console or API save still sending them.
        let saved = serde_json::json!({"rtx_hdr_saturation": 5, "rtx_hdr_peak_brightness": 900});
        config.update(saved.as_object().unwrap()).unwrap();
        assert!(!config.values.contains_key("rtx_hdr_saturation"));
        assert_eq!(config.integer("hdr_peak_brightness", 0), 900);
        // Devices and apps may still choose the peak, under its new name.
        assert!(super::override_allowed("hdr_peak_brightness"));
        assert!(!super::override_allowed("rtx_hdr"));
        assert_eq!(
            super::current_name("rtx_hdr_peak_brightness"),
            "hdr_peak_brightness"
        );
    }
    #[test]
    fn packet_pacing_requires_opt_in_and_yields_to_manual_bitrate_and_codec_policies() {
        let cases = [
            ("", [false, false, false, false]),
            ("host_adaptive_pacing=false", [false, false, false, false]),
            ("host_adaptive_pacing=invalid", [false, false, false, false]),
            ("host_adaptive_pacing=true", [true, true, true, true]),
            (
                "host_adaptive_pacing=true\npacing_max_bitrate_kbps=0",
                [true, true, true, true],
            ),
            (
                "host_adaptive_pacing=true\npacing_max_bitrate_kbps=-1",
                [true, true, true, true],
            ),
            (
                "host_adaptive_pacing=true\npacing_max_bitrate_kbps=1",
                [false, false, false, false],
            ),
            (
                "host_adaptive_pacing=true\npacing_max_bitrate_kbps=80000",
                [false, false, false, false],
            ),
            (
                "host_adaptive_pacing=true\nhost_adaptive_bitrate=true",
                [false, false, false, false],
            ),
            (
                "host_adaptive_pacing=true\nhost_adaptive_bitrate=false",
                [true, true, true, true],
            ),
        ];
        for (text, expected) in cases {
            let config = super::Config::parse(text).unwrap();
            for (codec, enabled) in expected.into_iter().enumerate() {
                assert_eq!(
                    config.adaptive_packet_pacing(codec as u8, false),
                    enabled,
                    "codec {codec}: {text}"
                );
            }
            assert!(!config.adaptive_packet_pacing(u8::MAX, false));
            assert!(!config.adaptive_packet_pacing(u8::MAX, true));
        }
    }

    #[test]
    fn a_wireless_client_gets_adaptive_pacing_on_h26x_and_av1_unless_something_owns_pacing() {
        // (config, [H.264, HEVC, AV1, PyroWave] adaptive, keyframe-loss pacing, same for wired)
        let on = [true, true, true, false];
        let off = [false; 4];
        let all = [true; 4];
        let cases = [
            ("", on, [false, false, false, true], off, [true; 4]),
            (
                "wireless_adaptive_pacing=true",
                on,
                [false, false, false, true],
                off,
                [true; 4],
            ),
            (
                "wireless_adaptive_pacing=invalid",
                on,
                [false, false, false, true],
                off,
                [true; 4],
            ),
            (
                "wireless_adaptive_pacing=false",
                off,
                [true; 4],
                off,
                [true; 4],
            ),
            // The host option still turns it on for everything, wired or wireless.
            (
                "host_adaptive_pacing=true",
                all,
                [false; 4],
                all,
                [false; 4],
            ),
            (
                "host_adaptive_pacing=true\nwireless_adaptive_pacing=false",
                all,
                [false; 4],
                all,
                [false; 4],
            ),
            // A manual limit or host bitrate adaptation owns pacing.
            (
                "pacing_max_bitrate_kbps=80000",
                off,
                [false; 4],
                off,
                [false; 4],
            ),
            (
                "pacing_max_bitrate_kbps=0",
                on,
                [false, false, false, true],
                off,
                [true; 4],
            ),
            (
                "host_adaptive_bitrate=true",
                off,
                [false; 4],
                off,
                [false; 4],
            ),
            // Keyframe-loss pacing can still be switched off.
            (
                "keyframe_loss_pacing=false",
                on,
                [false; 4],
                off,
                [false; 4],
            ),
        ];
        for (text, wireless, wireless_keyframe, wired, wired_keyframe) in cases {
            let config = super::Config::parse(text).unwrap();
            for codec in 0..4u8 {
                let i = usize::from(codec);
                assert_eq!(
                    config.adaptive_packet_pacing(codec, true),
                    wireless[i],
                    "wireless codec {codec}: {text}"
                );
                assert_eq!(
                    config.keyframe_loss_pacing(codec, true),
                    wireless_keyframe[i],
                    "wireless keyframe codec {codec}: {text}"
                );
                assert_eq!(
                    config.adaptive_packet_pacing(codec, false),
                    wired[i],
                    "wired codec {codec}: {text}"
                );
                assert_eq!(
                    config.keyframe_loss_pacing(codec, false),
                    wired_keyframe[i],
                    "wired keyframe codec {codec}: {text}"
                );
            }
        }
        // PyroWave plus a wireless client stays on its calibrated pace.
        let default = super::Config::default();
        assert!(!default.adaptive_packet_pacing(3, true));
        assert!(default.keyframe_loss_pacing(3, true));
        assert!(super::override_allowed("wireless_adaptive_pacing"));
    }

    #[test]
    fn disconnected_displays_restore_by_default_and_exclusive_layouts_cannot_be_retained() {
        use std::time::Duration;
        for (text, ordinary, exclusive) in [
            ("", Some(3000), Some(3000)),
            (
                "dd_config_revert_on_disconnect=false",
                Some(7_200_000),
                Some(3000),
            ),
            (
                "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=0",
                None,
                Some(3000),
            ),
            (
                "dd_config_revert_on_disconnect=false\ndd_paused_virtual_display_timeout_secs=20\ndd_config_revert_delay=500",
                Some(20_000),
                Some(500),
            ),
            ("dd_config_revert_delay=0", Some(0), Some(0)),
            ("dd_config_revert_delay=-1", Some(0), Some(0)),
            (
                "dd_config_revert_on_disconnect=invalid",
                Some(3000),
                Some(3000),
            ),
        ] {
            let config = super::Config::parse(text).unwrap();
            assert_eq!(
                config.display_disconnect_delay(false),
                ordinary.map(Duration::from_millis),
                "{text}"
            );
            assert_eq!(
                config.display_disconnect_delay(true),
                exclusive.map(Duration::from_millis),
                "{text}"
            );
        }
    }

    #[test]
    fn keyframe_loss_pacing_is_on_unless_a_manual_limit_or_adaptation_owns_pacing() {
        for (text, enabled) in [
            ("", true),
            ("keyframe_loss_pacing=true", true),
            ("keyframe_loss_pacing=invalid", true),
            ("keyframe_loss_pacing=false", false),
            ("pacing_max_bitrate_kbps=0", true),
            ("pacing_max_bitrate_kbps=80000", false),
            ("host_adaptive_pacing=true", false),
            ("host_adaptive_bitrate=true", false),
        ] {
            let config = super::Config::parse(text).unwrap();
            assert_eq!(config.keyframe_loss_pacing(0, false), enabled, "{text}");
        }
        assert!(super::override_allowed("keyframe_loss_pacing"));
    }

    #[test]
    fn link_rate_adaptation_is_on_unless_disabled_or_the_opt_in_owns_the_bitrate() {
        for (text, enabled) in [
            ("", true),
            ("link_rate_adaptation=true", true),
            ("link_rate_adaptation=invalid", true),
            ("link_rate_adaptation=false", false),
            ("host_adaptive_bitrate=true", false),
            (
                "host_adaptive_bitrate=true\nlink_rate_adaptation=true",
                false,
            ),
            ("host_adaptive_bitrate=false", true),
            ("host_adaptive_pacing=true", true),
        ] {
            let config = super::Config::parse(text).unwrap();
            assert_eq!(config.link_rate_adaptation(), enabled, "{text}");
        }
        assert!(super::override_allowed("link_rate_adaptation"));
    }

    #[test]
    fn resetting_packet_pacing_opt_in_restores_the_disabled_default() {
        let mut config = super::Config::default();
        assert!(!config.adaptive_packet_pacing(0, false));
        for reset in [serde_json::Value::Null, serde_json::json!("")] {
            config
                .update(
                    serde_json::json!({"host_adaptive_pacing": true})
                        .as_object()
                        .unwrap(),
                )
                .unwrap();
            assert!(config.adaptive_packet_pacing(0, false));
            config
                .update(
                    serde_json::json!({"host_adaptive_pacing": reset})
                        .as_object()
                        .unwrap(),
                )
                .unwrap();
            assert!(!config.adaptive_packet_pacing(0, false));
            assert!(!config.values.contains_key("host_adaptive_pacing"));
        }
    }

    #[test]
    fn hardware_metrics_defaults_and_file_interval_bounds() {
        let defaults = super::Config::default();
        assert!(!defaults.boolean("hardware_metrics", false));
        assert_eq!(defaults.hardware_metrics_interval(), 1000);
        assert!(defaults.list("hardware_metrics_selection").is_empty());
        for (value, expected) in [
            ("invalid", 1000),
            ("0", 500),
            ("750", 750),
            ("20000", 10000),
        ] {
            let config =
                super::Config::parse(&format!("hardware_metrics_interval={value}\n")).unwrap();
            assert_eq!(config.hardware_metrics_interval(), expected);
        }
    }
    #[test]
    fn stream_preset_saves_only_a_preset_name_and_clears_with_null() {
        let mut config = super::Config::default();
        for value in ["lowest_latency", "balanced", "reference", "Reference"] {
            change(&mut config, "stream_preset", value).unwrap();
            assert_eq!(config.get("stream_preset", ""), value);
        }
        for value in ["custom", "fastest", "lowest", "5"] {
            assert!(
                change(&mut config, "stream_preset", value).is_err(),
                "{value}"
            );
            assert_eq!(config.get("stream_preset", ""), "Reference");
        }
        let mut object = serde_json::Map::new();
        object.insert("stream_preset".into(), serde_json::Value::Null);
        config.update(&object).unwrap();
        assert!(!config.values.contains_key("stream_preset"));
        // A file the host did not write may hold anything; it still loads.
        let loaded = super::Config::parse(
            "stream_preset = fastest
",
        )
        .unwrap();
        assert_eq!(loaded.get("stream_preset", ""), "fastest");
    }
    #[test]
    fn hardware_metrics_saves_validate_interval_and_selection() {
        let mut config = super::Config::default();
        for value in ["500", "1000", "10000"] {
            change(&mut config, "hardware_metrics_interval", value).unwrap();
        }
        for value in ["499", "10001", "invalid", "1.5"] {
            assert!(change(&mut config, "hardware_metrics_interval", value).is_err());
        }
        for value in [
            serde_json::Value::Null,
            serde_json::Value::String(String::new()),
        ] {
            change(&mut config, "hardware_metrics_interval", "750").unwrap();
            let mut object = serde_json::Map::new();
            object.insert("hardware_metrics_interval".into(), value);
            config.update(&object).unwrap();
            assert!(!config.values.contains_key("hardware_metrics_interval"));
            assert_eq!(config.hardware_metrics_interval(), 1000);
        }
        change(
            &mut config,
            "hardware_metrics_selection",
            "[cpu_usage_percent, gpu_temperature_c]",
        )
        .unwrap();
        assert_eq!(
            config.list("hardware_metrics_selection"),
            ["cpu_usage_percent", "gpu_temperature_c"]
        );
        assert!(
            change(
                &mut config,
                "hardware_metrics_selection",
                "[cpu_usage_percent, cpu_temperature_c]"
            )
            .is_err()
        );
        assert_eq!(
            config.list("hardware_metrics_selection"),
            ["cpu_usage_percent", "gpu_temperature_c"]
        );
        change(&mut config, "hardware_metrics_selection", "[]").unwrap();
        assert!(config.list("hardware_metrics_selection").is_empty());
        let mut config = super::Config::parse(
            "hardware_metrics_interval=0\nhardware_metrics_selection=[unknown]\n",
        )
        .unwrap();
        change(&mut config, "sunshine_name", "Gaming PC").unwrap();
        assert_eq!(config.list("hardware_metrics_selection"), ["unknown"]);
    }
    #[test]
    fn a_saved_ltr_zero_from_before_2_0_0_gives_way_to_the_new_default_once() {
        let mut config = super::Config::parse("amd_ltr_frames = 0\nport = 47989\n").unwrap();
        assert!(config.upgrade_ltr_default(false));
        assert!(!config.values.contains_key("amd_ltr_frames"));
        assert_eq!(config.integer("port", 0), 47989);
        // Chosen after the upgrade, 0 stays off.
        let mut config = super::Config::parse("amd_ltr_frames = 0\n").unwrap();
        assert!(!config.upgrade_ltr_default(true));
        assert_eq!(config.integer("amd_ltr_frames", 4), 0);
        // Other values are the user's own.
        for text in ["", "amd_ltr_frames = 1\n", "amd_ltr_frames = 4\n"] {
            let mut config = super::Config::parse(text).unwrap();
            let before = config.values.clone();
            assert!(!config.upgrade_ltr_default(false), "{text}");
            assert_eq!(config.values, before);
        }
    }
    #[test]
    fn overrides_allow_stream_input_display_and_encoder_keys_but_not_host_settings() {
        for key in [
            "max_bitrate",
            "stream_audio",
            "keyboard",
            "gamepad",
            "dd_hdr_option",
            "dd_mode_remapping",
            "encoder",
            "amd_quality",
            "encoder_focus",
            "frame_limiter_fps_limit",
            "adaptive_fec",
            "host_adaptive_pacing",
        ] {
            assert!(super::override_allowed(key), "{key}");
        }
        for key in [
            "port",
            "bind_address",
            "file_apps",
            "cert",
            "pkey",
            "enable_pairing",
            "server_cmd",
            "global_prep_cmd",
            "log_path",
            "unknown",
        ] {
            assert!(!super::override_allowed(key), "{key}");
        }
    }
    #[test]
    fn audio_processing_overrides_per_device_and_app_but_idle_sleep_stays_host_wide() {
        assert!(super::override_allowed("audio_night_mode"));
        assert!(super::override_allowed("audio_headphone_surround"));
        assert!(!super::override_allowed("sleep_after_idle_minutes"));
        assert!(!super::override_allowed("sleep_while_suspended"));
    }
    #[test]
    fn idle_sleep_minutes_are_off_or_between_5_and_1440() {
        let mut config = super::Config::default();
        for value in ["0", "5", "30", "1440", "\"60\""] {
            change(&mut config, "sleep_after_idle_minutes", value).unwrap();
        }
        assert_eq!(config.integer("sleep_after_idle_minutes", -1), 60);
        for value in ["3", "4", "1441", "-5", "off", "1.5", "true"] {
            let error = change(&mut config, "sleep_after_idle_minutes", value).unwrap_err();
            assert!(
                error.to_string().contains("sleep_after_idle_minutes"),
                "{value}: {error}"
            );
        }
        assert_eq!(config.integer("sleep_after_idle_minutes", -1), 60);
        // A JSON number from the console is checked the same way.
        let mut object = serde_json::Map::new();
        object.insert("sleep_after_idle_minutes".into(), 3.into());
        assert!(config.update(&object).is_err());
        object.insert("sleep_after_idle_minutes".into(), 15.into());
        config.update(&object).unwrap();
        assert_eq!(config.integer("sleep_after_idle_minutes", -1), 15);
        // Null resets it to off.
        object.insert("sleep_after_idle_minutes".into(), serde_json::Value::Null);
        config.update(&object).unwrap();
        assert!(!config.values.contains_key("sleep_after_idle_minutes"));
    }
    #[test]
    fn audio_processing_and_suspended_sleep_settings_are_booleans() {
        for key in [
            "audio_night_mode",
            "audio_headphone_surround",
            "sleep_while_suspended",
        ] {
            let mut config = super::Config::default();
            for value in ["true", "on", "enabled", "1"] {
                change(&mut config, key, value).unwrap();
                assert!(config.boolean(key, false), "{key}={value}");
            }
            for value in ["false", "off", "0"] {
                change(&mut config, key, value).unwrap();
                assert!(!config.boolean(key, true), "{key}={value}");
            }
            for value in ["maybe", "2", "night"] {
                let error = change(&mut config, key, value).unwrap_err();
                assert!(error.to_string().contains(key), "{key}={value}: {error}");
            }
            let mut object = serde_json::Map::new();
            object.insert(key.into(), true.into());
            config.update(&object).unwrap();
            assert!(config.boolean(key, false), "{key} as a JSON boolean");
        }
    }
    #[test]
    fn audio_haptics_settings_are_checked_on_save_and_override_per_device() {
        use crate::audio_haptics::{CURVE, ENABLED, GATE, SETTINGS, STRENGTH};
        let mut config = super::Config::default();
        for (key, value) in [
            (ENABLED, "true"),
            (STRENGTH, "0"),
            (STRENGTH, "200"),
            (CURVE, "soft"),
            (GATE, "-80"),
            (GATE, "-10"),
        ] {
            change(&mut config, key, value).unwrap();
        }
        for (key, value) in [
            (ENABLED, "sometimes"),
            (STRENGTH, "250"),
            (CURVE, "wobbly"),
            (GATE, "0"),
        ] {
            let error = change(&mut config, key, value).unwrap_err();
            assert!(error.to_string().contains(key), "{key}={value}: {error}");
        }
        let mut object = serde_json::Map::new();
        object.insert(STRENGTH.into(), 300.into());
        assert!(config.update(&object).is_err());
        object.insert(STRENGTH.into(), serde_json::Value::Null);
        config.update(&object).unwrap();
        assert!(!config.values.contains_key(STRENGTH));
        for key in SETTINGS {
            assert!(super::override_allowed(key), "{key}");
        }
    }
    #[test]
    fn a_device_or_app_audio_override_beats_the_host_and_a_bad_one_is_refused() {
        // The host applies device, then app overrides onto its own file
        // through `update`, one key at a time.
        let mut config = super::Config::parse("audio_night_mode = off\n").unwrap();
        let layer = |config: &mut super::Config, key: &str, value: serde_json::Value| {
            config.update(&serde_json::Map::from_iter([(key.to_owned(), value)]))
        };
        layer(&mut config, "audio_night_mode", true.into()).unwrap();
        assert!(config.boolean("audio_night_mode", false));
        layer(&mut config, "audio_night_mode", "off".into()).unwrap();
        assert!(!config.boolean("audio_night_mode", true));
        assert!(layer(&mut config, "audio_headphone_surround", "loud".into()).is_err());
        assert!(!config.values.contains_key("audio_headphone_surround"));
    }
    #[test]
    fn couch_menu_controls_are_checked_on_save_with_one_message_each() {
        let mut config = super::Config::default();
        for (key, value) in [
            ("couch_menu_controller_chord", "l1+r1+y"),
            ("couch_menu_controller_chord", "guide"),
            ("couch_menu_hold_ms", "300"),
            ("couch_menu_hold_ms", "3000"),
            ("couch_menu_corner_hold_ms", "500"),
            ("couch_menu_corner_hold_ms", "3000"),
            ("couch_menu_keyboard_chord", "ctrl+alt+insert"),
            ("couch_menu_open_with", "controller,touch"),
        ] {
            change(&mut config, key, value).unwrap();
            assert_eq!(config.get(key, ""), value);
        }
        for (key, value, message) in [
            (
                "couch_menu_controller_chord",
                "start+select+l1+r1",
                "quits the stream",
            ),
            ("couch_menu_controller_chord", "start", "mouse mode"),
            ("couch_menu_controller_chord", "start+select", "into Guide"),
            (
                "couch_menu_controller_chord",
                "select+l1+r1+x",
                "statistics",
            ),
            ("couch_menu_controller_chord", "a+start", "mouse mode"),
            (
                "couch_menu_controller_chord",
                "l1+r1+a",
                "A presses the focused row",
            ),
            ("couch_menu_controller_chord", "b", "B goes back"),
            ("couch_menu_controller_chord", "x+up", "Up moves the focus"),
            ("couch_menu_controller_chord", "l3+r2", "unknown button"),
            ("couch_menu_hold_ms", "299", "between 300 and 3000"),
            ("couch_menu_hold_ms", "3001", "between 300 and 3000"),
            ("couch_menu_corner_hold_ms", "499", "between 500 and 3000"),
            ("couch_menu_corner_hold_ms", "3001", "between 500 and 3000"),
            (
                "couch_menu_keyboard_chord",
                "ctrl+alt+shift+home",
                "Ctrl+Alt+Shift",
            ),
            ("couch_menu_keyboard_chord", "ctrl+alt+del", "unknown chord"),
            ("couch_menu_open_with", "pad", "unknown entry"),
        ] {
            let error = change(&mut config, key, value).unwrap_err().to_string();
            assert!(error.starts_with(key), "{key}={value}: {error}");
            assert!(error.contains(message), "{key}={value}: {error}");
        }
        // Nothing refused was kept.
        assert_eq!(config.get("couch_menu_controller_chord", ""), "guide");
        assert_eq!(config.integer("couch_menu_hold_ms", 0), 3000);
        assert_eq!(config.integer("couch_menu_corner_hold_ms", 0), 3000);
        assert_eq!(
            config.get("couch_menu_keyboard_chord", ""),
            "ctrl+alt+insert"
        );
        // A JSON number from the console is checked the same way.
        let mut object = serde_json::Map::new();
        object.insert("couch_menu_hold_ms".into(), 299.into());
        assert!(config.update(&object).is_err());
        object.insert("couch_menu_hold_ms".into(), 750.into());
        config.update(&object).unwrap();
        let open_with = crate::couch_menu::chord::OpenWith::from_config(&config);
        assert_eq!(open_with.pad_hold, std::time::Duration::from_millis(750));
        assert!(!open_with.keyboard);
    }
    #[test]
    fn a_device_couch_menu_override_beats_the_host_and_a_bad_one_is_refused() {
        use crate::couch_menu::chord::{KeyChord, OpenWith, PadButtons};
        for key in [
            "couch_menu_open_with",
            "couch_menu_controller_chord",
            "couch_menu_hold_ms",
            "couch_menu_corner_hold_ms",
            "couch_menu_keyboard_chord",
        ] {
            assert!(super::override_allowed(key), "{key}");
        }
        let mut config = super::Config::parse(
            "couch_menu_controller_chord = l1+r1+y\ncouch_menu_hold_ms = 2000\n",
        )
        .unwrap();
        let layer = |config: &mut super::Config, key: &str, value: serde_json::Value| {
            config.update(&serde_json::Map::from_iter([(key.to_owned(), value)]))
        };
        layer(&mut config, "couch_menu_controller_chord", "select".into()).unwrap();
        layer(&mut config, "couch_menu_hold_ms", 400.into()).unwrap();
        layer(
            &mut config,
            "couch_menu_keyboard_chord",
            "ctrl+alt+end".into(),
        )
        .unwrap();
        layer(&mut config, "couch_menu_open_with", "controller".into()).unwrap();
        let open_with = OpenWith::from_config(&config);
        assert!(open_with.controller && !open_with.keyboard && !open_with.touch);
        assert_eq!(open_with.pad_chord, PadButtons::parse("select").unwrap());
        assert_eq!(open_with.pad_hold, std::time::Duration::from_millis(400));
        assert_eq!(
            open_with.key_chord,
            KeyChord::parse("ctrl+alt+end").unwrap()
        );
        assert!(layer(&mut config, "couch_menu_controller_chord", "start".into()).is_err());
        assert!(layer(&mut config, "couch_menu_corner_hold_ms", 100.into()).is_err());
        assert_eq!(config.get("couch_menu_controller_chord", ""), "select");
        assert!(!config.values.contains_key("couch_menu_corner_hold_ms"));
    }
    use super::*;
    /// A change saved to sunshine.conf, as the settings API does it.
    fn change(config: &mut Config, key: &str, value: &str) -> Result<()> {
        let mut object = serde_json::Map::new();
        object.insert(key.into(), value.into());
        let mut next = config.clone();
        next.update(&object)?;
        next.check_saved(object.keys())?;
        *config = next;
        Ok(())
    }
    #[test]
    fn overrides_that_are_never_saved_may_hold_a_comment_sign() {
        let mut config = Config::default();
        let mut object = serde_json::Map::new();
        object.insert("sunshine_name".into(), "Gaming PC #2".into());
        config.update(&object).unwrap();
        assert_eq!(config.get("sunshine_name", ""), "Gaming PC #2");
    }
    #[test]
    fn a_bad_value_already_in_the_file_does_not_block_other_saves() {
        let mut config = Config::parse("port = 70000\nframe_limiter_fps_limit = -1\n").unwrap();
        change(&mut config, "encoder", "amf").unwrap();
        assert_eq!(config.get("encoder", ""), "amf");
        assert!(change(&mut config, "port", "70000").is_err());
        change(&mut config, "port", "48000").unwrap();
    }
    #[test]
    fn values_the_file_cannot_give_back_are_refused() {
        let mut config = Config::default();
        // A comment would cut it short.
        assert!(change(&mut config, "sunshine_name", "Gaming PC #2").is_err());
        change(&mut config, "sunshine_name", "\"Gaming PC #2\"").unwrap();
        // An unclosed bracket would swallow the settings after it.
        assert!(change(&mut config, "adapter_name", "[Radeon").is_err());
        assert!(!config.values.contains_key("adapter_name"));
        // Spaces around a value and indented lists come back as the file reads them.
        change(&mut config, "sunshine_name", "  Living room  ").unwrap();
        change(&mut config, "global_prep_cmd", "[\n  {\"do\": \"a\"}\n]").unwrap();
        let reread = Config::parse(&config.text()).unwrap();
        assert_eq!(reread.get("sunshine_name", ""), "Living room");
        assert_eq!(reread.get("global_prep_cmd", ""), "[\n{\"do\": \"a\"}\n]");
    }
    #[test]
    fn bind_address_and_log_path_that_would_stop_the_host_are_refused() {
        let mut config = Config::default();
        assert!(change(&mut config, "bind_address", "Ethernet").is_err());
        change(&mut config, "bind_address", "192.168.1.5").unwrap();
        change(&mut config, "bind_address", "").unwrap();
        assert!(!config.values.contains_key("bind_address"));
        assert!(change(&mut config, "log_path", "C:\\Logs\\").is_err());
        let folder = std::env::temp_dir();
        assert!(change(&mut config, "log_path", folder.to_str().unwrap()).is_err());
        change(&mut config, "log_path", "D:\\Logs\\host.log").unwrap();
    }
    #[test]
    fn display_policies_keep_resolution_and_refresh_independent() {
        let config =
            Config::parse("dd_resolution_option = disabled\ndd_refresh_rate_option = auto\n")
                .unwrap();
        let requested = config.display_request(1920, 1080, 120).unwrap();
        assert_eq!(requested.resolution, None);
        assert_eq!(requested.refresh, Some(120));
        let config = Config::parse("dd_resolution_option = manual\ndd_manual_resolution = 2560x1440\ndd_refresh_rate_option = disabled\n").unwrap();
        let requested = config.display_request(1920, 1080, 120).unwrap();
        assert_eq!(requested.resolution, Some((2560, 1440)));
        assert_eq!(requested.refresh, None);
        for value in ["NaN", "inf", "0", "1001"] {
            let config = Config::parse(&format!(
                "dd_refresh_rate_option = manual\ndd_manual_refresh_rate = {value}\n"
            ))
            .unwrap();
            assert!(config.display_request(1920, 1080, 60).is_err());
        }
    }
    /// A profile from a host that streamed with NVIDIA, Intel or software
    /// encoding, or limited frames in the NVIDIA driver.
    const REMOVED_FEATURES: &str = "encoder = nvenc\nframe_limiter_provider = nvidia-control-panel\nrtss_frame_limit_type = nvidia-reflex\nnvenc_preset = 4\nqsv_preset = slow\nsw_preset = fast\nmin_threads = 4\nframe_limiter_disable_vsync = true\nrtss_disable_vsync_ullm = true\nnvenc_force_split_encode = forced\ndd_wa_dummy_plug_hdr10 = true\nport = 48123\n";
    #[test]
    fn removed_encoders_and_nvidia_limiters_migrate_with_one_report() {
        let (config, retired) = Config::parse_retiring(REMOVED_FEATURES).unwrap();
        assert_eq!(config.get("encoder", ""), "amf");
        assert_eq!(config.get("frame_limiter_provider", ""), "auto");
        assert_eq!(config.get("rtss_frame_limit_type", ""), "async");
        assert_eq!(config.port().unwrap(), 48123);
        // The removed encoders' and the NVIDIA driver's own settings go quietly.
        assert_eq!(config.values.len(), 4, "{:?}", config.values);
        assert_eq!(
            retired
                .iter()
                .map(|r| (r.key.as_str(), r.value.as_str(), r.current))
                .collect::<Vec<_>>(),
            [
                ("encoder", "nvenc", "amf"),
                ("frame_limiter_provider", "nvidia-control-panel", "auto"),
                ("rtss_frame_limit_type", "nvidia-reflex", "async"),
            ]
        );
        let warning = retired_warning(&retired).unwrap();
        assert!(warning.contains("encoder nvenc is now amf"), "{warning}");
        assert!(warning.contains("frame_limiter_provider nvidia-control-panel is now auto"));
        // What the host writes back reads without a report.
        let (again, retired) = Config::parse_retiring(&config.text()).unwrap();
        assert_eq!(again.values, config.values);
        assert!(retired.is_empty());
        assert!(retired_warning(&retired).is_none());
    }
    #[test]
    fn every_encoder_the_host_no_longer_has_becomes_amf() {
        for name in [
            "nvenc",
            "nvenc_experimental",
            "nvenc_legacy",
            "quicksync",
            "qsv",
            "software",
            "mediafoundation",
            "\"nvenc\"",
            "something_else",
        ] {
            let (config, retired) = Config::parse_retiring(&format!("encoder = {name}\n")).unwrap();
            assert_eq!(config.get("encoder", ""), "amf", "{name}");
            assert_eq!(retired.len(), 1, "{name}");
        }
        for kept in ["auto", "amf", "pyrowave", "amdvce", "amdvce_legacy"] {
            let (config, retired) = Config::parse_retiring(&format!("encoder = {kept}\n")).unwrap();
            assert_eq!(config.get("encoder", ""), kept);
            assert!(retired.is_empty(), "{kept}");
        }
        for (key, old, current) in [
            ("frame_limiter_provider", "nvcp", "auto"),
            ("frame_limiter_provider", "NVIDIA", "auto"),
            ("rtss_frame_limit_type", "reflex", "async"),
            ("rtss_frame_limit_type", "nvidia reflex", "async"),
        ] {
            let config = Config::parse(&format!("{key} = {old}\n")).unwrap();
            assert_eq!(config.get(key, ""), current, "{key} {old}");
        }
        for (key, kept) in [
            ("frame_limiter_provider", "rtss"),
            ("frame_limiter_provider", "none"),
            ("rtss_frame_limit_type", "back-edge-sync"),
        ] {
            let config = Config::parse(&format!("{key} = {kept}\n")).unwrap();
            assert_eq!(config.get(key, ""), kept);
        }
        // A save sending a removed encoder stores AMF.
        let mut config = Config::default();
        change(&mut config, "encoder", "nvenc").unwrap();
        assert_eq!(config.get("encoder", ""), "amf");
        // An app's or a device's own overrides.
        let mut app = serde_json::json!({"name": "Game", "frame-generation-mode": "nvidia-smooth-motion",
            "config-overrides": {"encoder": "software", "frame_limiter_provider": "nvidia-control-panel"}});
        assert!(replace_retired_in_app(app.as_object_mut().unwrap()));
        assert_eq!(
            app,
            serde_json::json!({"name": "Game", "frame-generation-mode": "none",
                "config-overrides": {"encoder": "amf", "frame_limiter_provider": "auto"}})
        );
        assert!(!replace_retired_in_app(app.as_object_mut().unwrap()));
    }
    #[test]
    fn retired_vigem_controllers_become_their_vhf_pads() {
        let c = Config::parse("gamepad = x360\n").unwrap();
        assert_eq!(c.get("gamepad", ""), "vhf_xbox_one");
        assert_eq!(c.text().trim(), "gamepad = vhf_xbox_one");
        let c = Config::parse("gamepad=ds4\n").unwrap();
        assert_eq!(c.get("gamepad", ""), "vhf_ds4");
        for kept in ["auto", "vhf_ds5", "ds5", "vhf_xbox"] {
            let c = Config::parse(&format!("gamepad={kept}\n")).unwrap();
            assert_eq!(c.get("gamepad", ""), kept);
        }
        // Only the gamepad setting has these retired values.
        assert_eq!(
            Config::parse("unknown_key=x360\n")
                .unwrap()
                .get("unknown_key", ""),
            "x360"
        );
        assert_eq!(replacement("gamepad", "x360"), Some("vhf_xbox_one"));
        assert_eq!(replacement("gamepad", "vhf_ds4"), None);
        // An app's or a device's own settings.
        let mut app = serde_json::json!({"name": "x360", "gamepad": "ds4", "output": "x360"});
        assert!(replace_retired(app.as_object_mut().unwrap()));
        assert_eq!(
            app,
            serde_json::json!({"name": "x360", "gamepad": "vhf_ds4", "output": "x360"})
        );
        assert!(!replace_retired(app.as_object_mut().unwrap()));
        let mut app = serde_json::json!({"name": "Game", "gamepad": "x360",
            "config-overrides": {"gamepad": "ds4", "fec_percentage": "30"}});
        assert!(replace_retired_in_app(app.as_object_mut().unwrap()));
        assert_eq!(
            app,
            serde_json::json!({"name": "Game", "gamepad": "vhf_xbox_one",
                "config-overrides": {"gamepad": "vhf_ds4", "fec_percentage": "30"}})
        );
        assert!(!replace_retired_in_app(app.as_object_mut().unwrap()));
    }
    /// A Vibepollo or Rubylight 2.0 file with every setting under its
    /// earlier name, a setting this host does not know, and a comment.
    const EARLIER_NAMES: &str = "# Vibepollo\nrtss_path = D:\\Tools\\RTSS\ndd_vdd_static_monitor_count = \"2\"\nrtx_hdr_peak_brightness = 1400\nport = 48123\nfuture_setting = kept\n";
    #[test]
    fn settings_under_an_earlier_name_are_read_under_the_current_one() {
        let c = Config::parse(EARLIER_NAMES).unwrap();
        // What the host used before through its fallbacks, now under the
        // names the console shows.
        assert_eq!(c.get("rtss_install_path", ""), "D:\\Tools\\RTSS");
        assert_eq!(c.integer("dd_virtual_display_permanent_count", -1), 2);
        assert_eq!(c.integer("hdr_peak_brightness", 0), 1400);
        for (old, _) in RENAMED_KEYS {
            assert!(!c.values.contains_key(*old), "{old}");
        }
        assert_eq!(c.get("future_setting", ""), "kept");
        assert_eq!(c.port().unwrap(), 48123);
        // With both names set the host read the current one; it stays.
        let both = Config::parse(
            "rtss_install_path = C:\\RTSS\nrtss_path = D:\\RTSS\ndd_virtual_display_permanent_count = 0\ndd_vdd_static_monitor_count = 3\n",
        )
        .unwrap();
        assert_eq!(both.get("rtss_install_path", ""), "C:\\RTSS");
        assert_eq!(both.integer("dd_virtual_display_permanent_count", -1), 0);
        assert_eq!(both.values.len(), 2);
        assert_eq!(current_name("rtss_path"), "rtss_install_path");
        assert_eq!(current_name("rtss_install_path"), "rtss_install_path");
        assert_eq!(current_name("port"), "port");
    }
    #[test]
    fn a_saved_configuration_holds_settings_under_their_current_names() {
        // The console saves every setting as held after parsing, so the
        // first save after an upgrade retires the earlier names.
        let before = Config::parse(EARLIER_NAMES).unwrap();
        let text = before.text();
        for (old, new) in RENAMED_KEYS {
            assert!(!text.contains(&format!("{old} =")), "{text}");
            assert!(text.contains(&format!("{new} =")), "{text}");
        }
        assert!(text.contains("future_setting = kept"), "{text}");
        assert_eq!(Config::parse(&text).unwrap().values, before.values);
    }
    #[test]
    fn a_setting_sent_under_an_earlier_name_is_saved_under_the_current_one() {
        let mut config = Config::parse("dd_virtual_display_permanent_count = 1\n").unwrap();
        change(&mut config, "dd_vdd_static_monitor_count", "3").unwrap();
        assert_eq!(config.integer("dd_virtual_display_permanent_count", 0), 3);
        assert!(!config.values.contains_key("dd_vdd_static_monitor_count"));
        // Sent together, the current name wins, as in the file.
        let mut object = serde_json::Map::new();
        object.insert("rtss_path".into(), "D:\\Old".into());
        object.insert("rtss_install_path".into(), "C:\\New".into());
        config.update(&object).unwrap();
        assert_eq!(config.get("rtss_install_path", ""), "C:\\New");
        assert!(!config.values.contains_key("rtss_path"));
        // Resetting the earlier name resets the setting.
        let mut object = serde_json::Map::new();
        object.insert("rtss_path".into(), serde_json::Value::Null);
        config.update(&object).unwrap();
        assert!(!config.values.contains_key("rtss_install_path"));
        // The value is checked under the name it is saved under.
        assert!(change(&mut config, "rtss_path", "C:\\RTSS #2").is_err());
        assert!(!config.values.contains_key("rtss_install_path"));
    }
    #[test]
    fn existing_config_round_trips() {
        let c = Config::parse("# Apollo\nport=48123\nunknown_key = custom\nprep_cmd = [\n {\"do\":\"echo #kept\",\"undo\":\"\"}\n]\n").unwrap();
        assert_eq!(c.ports().unwrap().rtsp, 48144);
        assert_eq!(Config::parse(&c.text()).unwrap().values, c.values);
        assert_eq!(c.get("unknown_key", ""), "custom");
        for (legacy, expected) in [
            ("debug", "debug"),
            ("1", "debug"),
            ("warning", "warn"),
            ("verbose", "trace"),
            ("fatal", "error"),
            ("none", "off"),
            ("invalid", "info"),
        ] {
            assert_eq!(
                Config::parse(&format!("min_log_level={legacy}\n"))
                    .unwrap()
                    .log_level(),
                expected
            );
        }
    }
    #[test]
    fn exact_display_remapping_hdr_override_and_disabled_policy() {
        let c = Config::parse("dd_mode_remapping={\"mixed\":[{\"requested_resolution\":\"1920x1080\",\"requested_fps\":\"59.94\",\"final_resolution\":\"2560x1440\",\"final_refresh_rate\":\"119.88\"}]}\ndd_hdr_request_override=force_off\n").unwrap();
        let r = c
            .display_request_rate(1920, 1080, crate::framegen::Rate(59940), true, false)
            .unwrap();
        assert_eq!(r.resolution, Some((2560, 1440)));
        assert_eq!(r.refresh, Some(119880));
        assert_eq!(r.hdr, Some(false));
        let c = Config::parse("dd_configuration_option=disabled\ndd_refresh_rate_option=manual\ndd_manual_refresh_rate=119.88\n").unwrap();
        let r = c
            .display_request_rate(1920, 1080, crate::framegen::Rate(59940), true, false)
            .unwrap();
        assert_eq!((r.resolution, r.refresh, r.hdr), (None, None, None));
        assert_eq!(
            c.display_request_rate(1920, 1080, crate::framegen::Rate(59940), true, true)
                .unwrap()
                .refresh,
            Some(119880)
        );
    }
    #[test]
    fn vibepollo_files_load_like_vibepollo() {
        let c = Config::parse(concat!(
            "\u{feff}sunshine_name = 27\" PC # living room\n",
            "not a setting\n",
            "keybindings = [0x10, 0xA0, \"0x11\", 0xA2]\n",
            "csrf_allowed_origins = [https://a.example, https://b.example]\n",
            "encoder =\n",
            "port = \"48123\"\n",
            "port = 50000\n",
            "audio_sink = Speakers\n",
            "bad key = 1\n",
            "global_prep_cmd = [\n",
            "  {\"do\":\"echo #1\",\"undo\":\"\"}\n",
            "]\n",
            "dd_snapshot_exclude_devices = DISPLAY1, \"DISPLAY2\"\n",
            "fec_percentage = 0x1e\n",
            "enable_pairing = enable\n",
        ))
        .unwrap();
        assert_eq!(c.get("sunshine_name", ""), "27\" PC");
        assert_eq!(c.list("keybindings"), ["0x10", "0xA0", "0x11", "0xA2"]);
        assert_eq!(
            c.list("csrf_allowed_origins"),
            ["https://a.example", "https://b.example"]
        );
        assert_eq!(c.get("encoder", "auto"), "auto");
        assert!(!c.values.contains_key("encoder"));
        assert_eq!(c.port().unwrap(), 48123);
        assert_eq!(c.get("audio_sink", ""), "Speakers");
        assert!(!c.values.contains_key("bad key") && !c.values.contains_key("key"));
        assert_eq!(
            c.get("global_prep_cmd", ""),
            "[\n{\"do\":\"echo #1\",\"undo\":\"\"}\n]"
        );
        assert_eq!(
            c.list("dd_snapshot_exclude_devices"),
            ["DISPLAY1", "DISPLAY2"]
        );
        assert_eq!(c.integer("fec_percentage", 20), 30);
        assert!(c.boolean("enable_pairing", false));
        assert_eq!(
            Config::parse("port = 70000\n").unwrap().port().unwrap(),
            47989
        );
        // An unclosed list is dropped; the rest of the file still loads.
        let c = Config::parse("keybindings = [0x10,\nport = 48123\n").unwrap();
        assert_eq!(c.port().unwrap(), 48123);
        assert!(!c.values.contains_key("keybindings"));
        let c = Config::parse("port = 48123\nkeybindings = [0x10,\n").unwrap();
        assert_eq!(c.port().unwrap(), 48123);
        assert!(!c.values.contains_key("keybindings"));
    }
    #[test]
    fn files_saved_as_utf16_or_in_the_ansi_code_page_load() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("sunshine.conf");
        let text = "sunshine_name = Café € PC\r\nport = 48123 # base\r\n";
        let mut utf16 = vec![0xff, 0xfe];
        utf16.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let mut big_endian = vec![0xfe, 0xff];
        big_endian.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        let mut bom = b"\xef\xbb\xbf".to_vec();
        bom.extend_from_slice(text.as_bytes());
        let ansi = b"sunshine_name = Caf\xe9 \x80 PC\r\nport = 48123 # base\r\n".to_vec();
        for bytes in [utf16, big_endian, bom, ansi, text.as_bytes().to_vec()] {
            std::fs::write(&path, bytes).unwrap();
            let c = Config::load(&path).unwrap();
            assert_eq!(c.get("sunshine_name", ""), "Café € PC");
            assert_eq!(c.port().unwrap(), 48123);
            assert_eq!(c.values.len(), 2);
        }
    }
    #[test]
    fn update_is_transactional_and_rejects_injection() {
        let mut c = Config::default();
        assert!(
            c.update(
                serde_json::json!({"port":65535,"encoder":"amf"})
                    .as_object()
                    .unwrap()
            )
            .is_err()
        );
        assert!(c.values.is_empty());
        assert!(
            c.update(
                serde_json::json!({"encoder":"amf\nport=1"})
                    .as_object()
                    .unwrap()
            )
            .is_err()
        );
    }
}
