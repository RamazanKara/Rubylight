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
pub use settings30::{DisplayAdjust, SETTINGS_30, TouchMode};

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
    RetiredSetting {
        key: crate::nucleus::mode::KEY,
        retired: crate::nucleus::mode::retired_background,
        current: "nucleus",
        reason: "Windows child sessions were removed: Windows signs a Microsoft account that uses only Windows Hello into one only with a password; Nucleus Co-op is the background mode",
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
    // The removed background child session.
    "child_session_resolution",
    "child_session_scale",
    "child_session_port",
    "child_session_host",
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
/// Apollo and Vibepollo store them, its `config-overrides` and those of its
/// devices' blocks (`device-overrides`). Returns whether anything changed.
pub fn replace_retired_in_app(app: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let mut changed = replace_retired(app.iter_mut());
    if let Some(overrides) = app
        .get_mut("config-overrides")
        .and_then(serde_json::Value::as_object_mut)
    {
        changed |= replace_retired(overrides.iter_mut());
    }
    for block in app
        .get_mut(crate::device_overrides::SETTING)
        .and_then(serde_json::Value::as_object_mut)
        .into_iter()
        .flat_map(|blocks| blocks.values_mut())
    {
        if let Some(overrides) = block
            .get_mut(crate::device_overrides::CONFIG)
            .and_then(serde_json::Value::as_object_mut)
        {
            changed |= replace_retired(overrides.iter_mut());
        }
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
    /// The Dolby Vision offer on HEVC and AV1 HDR streams, for devices that
    /// pick it (`hdr_dolby_vision`, on by default; `extensions::hdr_offer`):
    /// 8.1 on HEVC, and 8.4 while `hdr_hlg` is on too; 10.1 on AV1.
    pub fn hdr_dolby_vision(&self) -> bool {
        self.boolean("hdr_dolby_vision", true)
    }
    /// The HLG offer on HEVC and AV1 HDR streams, for devices that pick it
    /// (`hdr_hlg`, on by default; `extensions::hdr_offer`).
    pub fn hdr_hlg(&self) -> bool {
        self.boolean("hdr_hlg", true)
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
    /// Images on the shared clipboard, besides text (on by default,
    /// Experimental). A device still needs the clipboard's permission bits.
    pub fn clipboard_image(&self) -> bool {
        self.boolean("clipboard_image", true)
    }
    /// The largest clipboard image in bytes (`clipboard_image_max_mb`, MiB,
    /// 1 to 64, 16 by default).
    pub fn clipboard_image_max_bytes(&self) -> u64 {
        let mut mib = self.integer("clipboard_image_max_mb", 16);
        if !(1..=64).contains(&mib) {
            invalid(
                "clipboard_image_max_mb",
                self.get("clipboard_image_max_mb", ""),
            );
            mib = 16;
        }
        mib as u64 * 1024 * 1024
    }
    /// `shared_folders` (Experimental): a JSON list of id, name and path. A
    /// list that isn't that, repeats or misspells an id, or names a path that
    /// isn't a folder on a drive of this PC is reported, and none is shared.
    pub fn shared_folders(&self) -> Vec<SharedFolderConfig> {
        let value = self.get("shared_folders", "[]");
        match parse_shared_folders(value) {
            Ok(folders) => folders,
            Err(error) => {
                refused("shared_folders", value, &error);
                Vec::new()
            }
        }
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
        if changed(crate::nucleus::mode::KEY) {
            crate::nucleus::mode::parse(&next.values[crate::nucleus::mode::KEY])
                .map_err(anyhow::Error::msg)?;
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
        "dd_hold_layout_while_paused",
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
/// reads as a path, and none of `__proto__`, `constructor` and `prototype`.
pub fn valid_folder_id(id: &str) -> bool {
    // Grants are a JSON object the console keeps as a JavaScript one, where
    // those names reach the prototype rather than a grant.
    !matches!(id, "__proto__" | "constructor" | "prototype")
        && (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
/// Whether `path` can be a shared folder: absolute, on a drive of this PC,
/// such as `D:\Games`. A network share (`\\server\share`), a device or
/// verbatim path (`\\.\`, `\\?\`), a relative path, `.` or `..`, and a
/// colon past the drive (an alternate data stream) are refused.
pub fn valid_folder_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
        && !path[2..].contains(':')
        && !path[3..]
            .split(['\\', '/'])
            .any(|part| part == "." || part == "..")
}

/// One entry of `shared_folders`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedFolderConfig {
    /// What device grants name; see [`valid_folder_id`].
    pub id: String,
    /// What the console and devices call the folder.
    pub name: String,
    /// See [`valid_folder_root`].
    pub path: PathBuf,
}

fn parse_shared_folders(text: &str) -> Result<Vec<SharedFolderConfig>, String> {
    let folders: Vec<SharedFolderConfig> = serde_json::from_str(text)
        .map_err(|e| format!("shared_folders is not a JSON list of folders: {e}"))?;
    let mut ids = std::collections::BTreeSet::new();
    for folder in &folders {
        if !valid_folder_id(&folder.id) {
            return Err(format!(
                "shared folder id {:?} is not a plain name",
                folder.id
            ));
        }
        if !ids.insert(folder.id.as_str()) {
            return Err(format!("shared folder id {:?} is listed twice", folder.id));
        }
        if folder.name.trim().is_empty() || folder.path.as_os_str().is_empty() {
            return Err(format!("shared folder {} has no name or path", folder.id));
        }
        if !folder.path.to_str().is_some_and(valid_folder_root) {
            return Err(format!(
                "shared folder {} is not a folder on a drive of this PC: {}",
                folder.id,
                folder.path.display()
            ));
        }
    }
    Ok(folders)
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
mod tests;
