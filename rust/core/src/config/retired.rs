//! Settings and values earlier releases and other hosts saved that the
//! host no longer reads, and what replaces each.
use super::unquote;

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
pub(super) fn retirement(key: &str, value: &str) -> Option<(&'static str, &'static str)> {
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
