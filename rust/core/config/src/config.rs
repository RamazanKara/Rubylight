//! The host configuration: `sunshine.conf`-compatible `key = value` settings,
//! typed accessors with Vibepollo's defaults, per-app and per-client overrides,
//! and the port layout derived from the base port.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

mod catalog_gen;
mod console_global_gen;
mod console_layers_gen;
mod retired_gen;
mod save;
mod settings30;
mod types;
pub use catalog_gen::{HARDWARE_METRICS, RESTART_KEYS};
pub use settings30::{DisplayAdjust, SETTINGS_30, TouchMode};
pub use types::{
    BitrateAdaptation, DSU_DEFAULT_PORT, FEC_MAX_PERCENTAGE, KeyboardLayoutSetting,
    MAX_STREAM_RESOLUTION, MAX_STREAM_RESOLUTION_NATIVE, MAX_STREAM_RESOLUTION_RANGE,
    MIC_PORT_OFFSET, PacketPacing, Ports, QuickSuspendMode, RemoveRemoteMonitor, UpdatesMode,
    WEBRTC_MEDIA_PORT, WEBRTC_PORT, parse_klid, parse_max_stream_resolution, parse_resolution,
};

/// Generated fallback-console setting tables and choice labels (`settings.toml`).
pub mod console_catalog {
    pub use super::console_global_gen::{Control, GLOBAL, GRIPS, Setting};
    pub use super::console_layers_gen::{APP, CLIENT, choice_label};
}

mod folders;
mod overrides;
mod retired;
mod text;
mod values;
use folders::parse_shared_folders;
pub use folders::{SharedFolderConfig, valid_folder_id, valid_folder_root};
pub use overrides::override_allowed;
use retired::retirement;
pub use retired::{
    AUTO_UPDATE, BACKGROUND_MODE_KEY, BITRATE_ADAPTATION, DD_DISCONNECT_BEHAVIOR,
    DD_REVERT_ON_DISCONNECT, DEVICE_OVERRIDES_CONFIG, DEVICE_OVERRIDES_SETTING, FOLDED_KEYS,
    HDR_OPTION, HDR_REQUEST, HOST_PACING, LIBRARY_SYNC_OPTIONS, LINK_RATE, MIN_QP, MIN_QP_SWITCH,
    PACING_RATE, PACKET_PACING, REMOTE_CLIENT_DISCONNECT, REMOTE_STREAM_END, REMOVE_REMOTE_MONITOR,
    REMOVED_APP_KEYS, REMOVED_KEYS, REMOVED_PREFIXES, RENAMED_APP_SWITCHES, RENAMED_KEYS,
    RETIRED_SETTINGS, RETIRED_VALUES, Retired, RetiredSetting, UPDATE_CHECK_INTERVAL, UPDATES,
    WIRELESS_PACING, current_name, fold_bitrate_adaptation, fold_bitrate_adaptation_decided,
    fold_disconnect_behavior, fold_library_sync, fold_min_qp, fold_min_qp_values,
    fold_packet_pacing, fold_packet_pacing_decided, fold_pyrowave_fec, fold_remove_remote_monitor,
    fold_remove_remote_monitor_decided, fold_updates, folded_bitrate_adaptation,
    folded_bitrate_adaptation_below, folded_disconnect_behavior, folded_hdr, folded_in_layer,
    folded_packet_pacing, folded_packet_pacing_below, folded_remove_remote_monitor,
    folded_remove_remote_monitor_below, folded_updates, holds_retired_settings, migrate_app,
    migrate_overrides, min_qp_switch_on, removed, replace_retired, replacement,
    retired_background_mode, retired_encoder, retired_frame_generation,
    retired_frame_limiter_provider, retired_name, retired_warning,
};
use text::{bracket_depth, decode, strip_comment};
pub use values::{
    fallback, first_report, invalid, number_text, parse_boolean, parse_integer, parse_time_of_day,
    refused, unquote,
};

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
        // Whether an earlier release wrote this file, before its old names go.
        let old_file = holds_retired_settings(&self.values);
        for &(old, new) in RENAMED_KEYS {
            if let Some(value) = self.values.remove(old)
                && !self.values.contains_key(new)
            {
                self.values.insert(new.to_owned(), value);
            }
        }
        // Each launcher's sync options fold into the shared Library sync
        // settings; a launcher that differs keeps its key as an override.
        fold_library_sync(&mut self.values, old_file);
        if let Some(request) = self.values.remove(HDR_REQUEST)
            && let Some(folded) = folded_in_layer(
                self.values.get(HDR_OPTION).map_or("auto", String::as_str),
                &request,
            )
        {
            self.values.insert(HDR_OPTION.to_owned(), folded.to_owned());
        }
        // The two bitrate switches fold into Adaptive bitrate; a value
        // already saved under it wins.
        let link = self.values.remove(retired::LINK_RATE);
        let congestion = self.values.remove(retired::CONGESTION);
        if let Some(folded) = folded_bitrate_adaptation(link.as_deref(), congestion.as_deref())
            && !self.values.contains_key(BITRATE_ADAPTATION)
        {
            self.values
                .insert(BITRATE_ADAPTATION.to_owned(), folded.to_owned());
        }
        // A PyroWave share equal to the error correction share is that share.
        if let Some(value) = self.values.get(retired::PYROWAVE_FEC)
            && retired::pyrowave_fec_is_default(
                Some(value),
                self.values.get("fec_percentage").map(String::as_str),
            )
        {
            self.values.remove(retired::PYROWAVE_FEC);
        }
        // The pacing rate and the two adaptive pacing switches fold into
        // Packet pacing; a value already saved under it wins.
        let rate = self.values.remove(retired::PACING_RATE);
        let host = self.values.remove(retired::HOST_PACING);
        let wireless = self.values.remove(retired::WIRELESS_PACING);
        if let Some(folded) =
            folded_packet_pacing(rate.as_deref(), host.as_deref(), wireless.as_deref())
            && !self.values.contains_key(PACKET_PACING)
        {
            self.values.insert(PACKET_PACING.to_owned(), folded);
        }
        // The remote monitor's two switches fold into Remove remote monitor;
        // a value already saved under it wins.
        let stream_end = self.values.remove(REMOTE_STREAM_END);
        let client_disconnect = self.values.remove(REMOTE_CLIENT_DISCONNECT);
        if let Some(folded) =
            folded_remove_remote_monitor(stream_end.as_deref(), client_disconnect.as_deref())
            && !self.values.contains_key(REMOVE_REMOTE_MONITOR)
        {
            self.values
                .insert(REMOVE_REMOTE_MONITOR.to_owned(), folded.to_owned());
        }
        // Restore displays on disconnect folds into the disconnect behavior;
        // a value already saved wins.
        let revert = self.values.remove(DD_REVERT_ON_DISCONNECT);
        if let Some(folded) = folded_disconnect_behavior(revert.as_deref())
            && !self.values.contains_key(DD_DISCONNECT_BEHAVIOR)
        {
            self.values
                .insert(DD_DISCONNECT_BEHAVIOR.to_owned(), folded.to_owned());
        }
        // Install updates automatically and an interval of 0 fold into
        // Updates; a value already saved under it wins.
        let auto_update = self.values.remove(AUTO_UPDATE);
        let interval = self.values.get(UPDATE_CHECK_INTERVAL).cloned();
        if let Some(folded) = folded_updates(interval.as_deref(), auto_update.as_deref()) {
            if folded == "off" {
                self.values.remove(UPDATE_CHECK_INTERVAL);
            }
            self.values
                .entry(UPDATES.to_owned())
                .or_insert_with(|| folded.to_owned());
        }
        // The Minimum quantizer switch folds into the minimum QPs: on keeps
        // them (the old defaults where none is saved), off clears them. An
        // earlier file without the switch had it off.
        fold_min_qp_values(&mut self.values, old_file);
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
    /// Whether a disconnected device's display is kept while its app runs
    /// (`dd_disconnect_behavior=keep_while_paused`) instead of restored after
    /// the restore delay (`restore`, the default). An unknown value reads as
    /// `restore`.
    pub fn keeps_display_while_paused(&self) -> bool {
        let value = self.get(DD_DISCONNECT_BEHAVIOR, "restore");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "keep_while_paused" => true,
            "restore" => false,
            _ => {
                invalid(DD_DISCONNECT_BEHAVIOR, value);
                false
            }
        }
    }
    /// How long a disconnected stream may keep its display. Exclusive layouts
    /// must release even when an older profile disables restoration: otherwise
    /// the local user's physical displays can remain off for hours. Keeping the
    /// monitors off while paused (`dd_hold_layout_while_paused`, which no older
    /// profile carries) is the user's choice to keep them off: such a layout keeps
    /// its display as long as an ordinary one would (issue #92).
    pub fn display_disconnect_delay(&self, exclusive: bool) -> Option<std::time::Duration> {
        let forced = exclusive && !self.boolean("dd_hold_layout_while_paused", false);
        if forced || !self.keeps_display_while_paused() {
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
    /// Packet pacing: `auto` (the default: adaptive pacing for a client that
    /// reports a Wi-Fi or cellular route on H.264, HEVC and AV1), `always`
    /// (every client and codec, PyroWave included), `off`, or a fixed rate
    /// in kbps. An unreadable value is reported and reads as `auto`; a rate
    /// of 0 or below is no rate, as the earlier pacing limit read it.
    pub fn packet_pacing(&self) -> PacketPacing {
        let value = self.get(PACKET_PACING, "auto");
        match unquote(value.trim()).to_ascii_lowercase().as_str() {
            "auto" => PacketPacing::Auto,
            "always" => PacketPacing::Always,
            "off" => PacketPacing::Off,
            _ => match parse_integer(value) {
                Some(rate) if rate > 0 => PacketPacing::Fixed(rate),
                // A rate of 0 or below is no rate, as the earlier pacing
                // limit read it.
                Some(_) => PacketPacing::Auto,
                None => {
                    invalid(PACKET_PACING, value);
                    PacketPacing::Auto
                }
            },
        }
    }
    /// Whether a remote monitor is removed when its stream ends: for
    /// `stream_end`, and for `disconnect`, which removes it then and when the
    /// device disconnects (both earlier switches on removed it at both).
    pub fn removes_remote_monitor_at_stream_end(&self) -> bool {
        self.remove_remote_monitor() != RemoveRemoteMonitor::Never
    }
    /// Whether the stream's packet pacing adapts to the link, by Packet
    /// pacing: `always` for every client on every codec (codec 0-3, PyroWave
    /// included), `auto` for a client that reports a Wi-Fi or cellular route
    /// (`client_wireless`) on H.264, HEVC and AV1 (codec 0-2) while PyroWave
    /// keeps its calibrated-pace policy, and never for `off` or a fixed rate.
    pub fn adaptive_packet_pacing(&self, codec: u8, client_wireless: bool) -> bool {
        match self.packet_pacing() {
            PacketPacing::Always => codec <= 3,
            PacketPacing::Auto => client_wireless && codec <= 2,
            PacketPacing::Off | PacketPacing::Fixed(_) => false,
        }
    }
    /// The fixed packet pacing rate in kbps, `None` when Packet pacing is
    /// not a positive rate. A fixed rate owns pacing: adaptive pacing and
    /// the loss latch both stay off.
    pub fn manual_pacing_kbps(&self) -> Option<i64> {
        match self.packet_pacing() {
            PacketPacing::Fixed(rate) => Some(rate),
            _ => None,
        }
    }
    /// Pace like Wi-Fi after a lost keyframe or repeated frame loss (on by default). A
    /// manual pacing rate keeps its own policy, and so does a stream the
    /// adaptive pacing controller owns.
    pub fn keyframe_loss_pacing(&self, codec: u8, client_wireless: bool) -> bool {
        self.boolean("keyframe_loss_pacing", true)
            && self.manual_pacing_kbps().is_none()
            && !self.adaptive_packet_pacing(codec, client_wireless)
    }
    /// Lower the encoder bitrate when the link cannot carry the stream:
    /// Adaptive bitrate on, or on with network delay.
    pub fn link_rate_adaptation(&self) -> bool {
        self.bitrate_adaptation() != BitrateAdaptation::Off
    }
    /// Packet-level congestion control from the client's arrival feedback: Adaptive bitrate's
    /// `delay` (not the default until the A/B decides).
    pub fn congestion_control(&self) -> bool {
        self.bitrate_adaptation() == BitrateAdaptation::Delay
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
    /// `virtual_display_mode`; unset, it is per client where the OS ships the
    /// IddCx the driver needs (`os_gate::virtual_display`, Windows 11 23H2 and
    /// later) and disabled before that, as Vibepollo does on Windows 10.
    pub fn virtual_display_mode(&self, windows_11: bool) -> &str {
        self.get(
            "virtual_display_mode",
            if windows_11 { "per_client" } else { "disabled" },
        )
    }
    /// `max_stream_resolution`: the host-wide cap on the picture every stream
    /// is encoded at, `None` for `native` (the default), which codes what each
    /// device asks for. A value the console would refuse is reported and
    /// behaves as native.
    pub fn max_stream_resolution(&self) -> Option<(u32, u32)> {
        let value = self.get(MAX_STREAM_RESOLUTION, MAX_STREAM_RESOLUTION_NATIVE);
        parse_max_stream_resolution(value)
            .inspect_err(|_| invalid(MAX_STREAM_RESOLUTION, value))
            .ok()
            .flatten()
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
    /// The PyroWave error-correction share, in percent: `fec_percentage`, as
    /// for every codec, unless a PyroWave value that differed from it was
    /// kept (`pyrowave_critical_fec_percentage`, [`retired::PYROWAVE_FEC`]).
    pub fn pyrowave_fec_percentage(&self) -> i64 {
        if self.values.contains_key(retired::PYROWAVE_FEC) {
            self.integer(retired::PYROWAVE_FEC, 20)
        } else {
            self.integer("fec_percentage", 20)
        }
    }
    /// Seconds between automatic update checks (`update_check_interval`, a day
    /// unless set); 0 or less is no check, as is Updates `off`.
    pub fn update_check_interval(&self) -> i64 {
        if self.updates() == UpdatesMode::Off {
            0
        } else {
            self.integer(UPDATE_CHECK_INTERVAL, 86400)
        }
    }
    /// `update_check_time`: the time of day (`HH:MM` on this PC's clock) of
    /// the daily automatic update check, as seconds after midnight. `None`
    /// when it is unset, and for text that is not a time (reported once):
    /// the check then follows `update_check_interval`.
    pub fn update_check_time(&self) -> Option<u32> {
        let value = self
            .values
            .get("update_check_time")
            .filter(|v| !v.trim().is_empty())?;
        let time = parse_time_of_day(value);
        if time.is_none() {
            invalid("update_check_time", value);
        }
        time
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
    /// How fast uploads are written while a stream runs, in bytes per
    /// second (`file_transfer_stream_mbps`, 50 by default). 0 or less does
    /// not slow them.
    pub fn file_transfer_stream_rate(&self) -> Option<u64> {
        let mbps = self.integer("file_transfer_stream_mbps", 50);
        (mbps > 0).then(|| (mbps as u64).saturating_mul(125_000))
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
    /// The DSU (cemuhook) motion server's UDP port when `enable_dsu_server`
    /// is on (off by default). `dsu_server_port` defaults to 26760, Foundation
    /// Sunshine's; a value outside 1024..=65535 is reported and that is used.
    pub fn dsu_server_port(&self) -> Option<u16> {
        self.boolean("enable_dsu_server", false)
            .then(|| self.webrtc_port_setting("dsu_server_port", DSU_DEFAULT_PORT))
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
    pub fn path(&self, key: &str, directory: &Path, default: &str) -> PathBuf {
        let p = PathBuf::from(self.get(key, default));
        if p.is_absolute() {
            p
        } else {
            directory.join(p)
        }
    }
}
/// Base ports whose derived ports (base - 5 to base + 21) are all valid.
const PORTS: std::ops::RangeInclusive<i64> = 1029..=65514;
/// Range a saved WebRTC or DSU port may take.
const WEBRTC_PORTS: std::ops::RangeInclusive<i64> = 1024..=65535;
#[cfg(test)]
mod tests;
