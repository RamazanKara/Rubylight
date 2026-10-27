//! The settings of the 3.0 extensions, defined ahead of their features: the
//! console lists them in `web/src/lib/schema/extensions.ts`, hidden until
//! each workstream ships its feature. A value the
//! host can't use is reported through [`invalid`] and the default applies,
//! so a launch never fails because of one of these settings.
use super::{Config, PORTS, first_report, invalid, refused, unquote, valid_folder_id};
use serde::Deserialize;
use std::path::PathBuf;

/// Every 3.0 setting with its default, as `sunshine.conf` would spell it.
pub const SETTINGS_30: &[(&str, &str)] = &[
    ("hdr_hlg", "true"),
    ("audio_surround_714", "true"),
    ("pen_barrel_roll", "true"),
    ("touch_keyboard_digitizer", "false"),
    ("touch_mode", "client"),
    ("touch_sensitivity", "100"),
    ("touch_gestures", "true"),
    ("client_cursor", "true"),
    ("clipboard_image", "true"),
    ("clipboard_image_max_mb", "16"),
    ("shared_folders", "[]"),
    ("webhooks", "[]"),
    ("qr_pairing", "false"),
    ("dd_rotation_option", "disabled"),
    ("dd_manual_rotation", "0"),
    ("dd_scale_option", "disabled"),
    ("dd_manual_scale", "100"),
    ("background_mode", "auto"),
    ("child_session_resolution", "1920x1080"),
    ("child_session_scale", "100"),
    ("child_session_port", "48289"),
];

/// `touch_mode`: how touches reach the PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchMode {
    /// As the device sends them; `native_pen_touch` still applies.
    Client,
    Mouse,
    Trackpad,
}

/// `dd_rotation_option` and `dd_scale_option`, beside `dd_resolution_option`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayAdjust {
    /// Leave the display as it is.
    Disabled,
    /// Match the device.
    Auto,
    /// Use the `dd_manual_*` value.
    Manual,
}

/// `background_mode`: where a stream beside the signed-in user runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundMode {
    Auto,
    ChildSession,
    VirtualDisplay,
}

/// One entry of `shared_folders`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedFolderConfig {
    /// What device grants name; see [`valid_folder_id`].
    pub id: String,
    /// What the console and devices call the folder.
    pub name: String,
    pub path: PathBuf,
}

impl Config {
    /// The setting's text, or `default` when it is unset or empty.
    fn text30<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        unquote(self.get(key, default).trim()).trim()
    }
    /// The value of `key` among `choices`, or the first choice's value.
    fn choice30<T: Copy>(&self, key: &str, choices: &[(&str, T)]) -> T {
        let value = self.text30(key, choices[0].0);
        match choices.iter().find(|(name, _)| *name == value) {
            Some(&(_, choice)) => choice,
            None => {
                invalid(key, value);
                choices[0].1
            }
        }
    }
    /// The integer `key` when `accept` takes it, otherwise `default`.
    fn integer30(&self, key: &str, default: i64, accept: impl Fn(i64) -> bool) -> i64 {
        let value = self.integer(key, default);
        if accept(value) {
            value
        } else {
            invalid(key, self.get(key, ""));
            default
        }
    }

    /// HLG HDR for devices that ask for it (on by default).
    pub fn hdr_hlg(&self) -> bool {
        self.boolean("hdr_hlg", true)
    }
    /// 7.1.4 surround sound (on by default).
    pub fn audio_surround_714(&self) -> bool {
        self.boolean("audio_surround_714", true)
    }
    /// Stylus barrel roll, 0x5500000B (on by default).
    pub fn pen_barrel_roll(&self) -> bool {
        self.boolean("pen_barrel_roll", true)
    }
    /// Let the Windows touch keyboard open for a device's touches (off by default).
    pub fn touch_keyboard_digitizer(&self) -> bool {
        self.boolean("touch_keyboard_digitizer", false)
    }
    /// `touch_mode`: `client` (the default), `mouse` or `trackpad`.
    pub fn touch_mode(&self) -> TouchMode {
        self.choice30(
            "touch_mode",
            &[
                ("client", TouchMode::Client),
                ("mouse", TouchMode::Mouse),
                ("trackpad", TouchMode::Trackpad),
            ],
        )
    }
    /// Pointer movement for a swipe in mouse or trackpad mode, in percent
    /// (25 to 400, 100 by default).
    pub fn touch_sensitivity(&self) -> u16 {
        self.integer30("touch_sensitivity", 100, |n| (25..=400).contains(&n)) as u16
    }
    /// Multi-finger touch gestures as scrolling and clicks (on by default).
    pub fn touch_gestures(&self) -> bool {
        self.boolean("touch_gestures", true)
    }
    /// Let a device draw the cursor itself (on by default).
    pub fn client_cursor(&self) -> bool {
        self.boolean("client_cursor", true)
    }
    /// Images on the shared clipboard, besides text (on by default).
    pub fn clipboard_image(&self) -> bool {
        self.boolean("clipboard_image", true)
    }
    /// The largest clipboard image in bytes (`clipboard_image_max_mb`, MiB,
    /// 1 to 64, 16 by default).
    pub fn clipboard_image_max_bytes(&self) -> u64 {
        let mib = self.integer30("clipboard_image_max_mb", 16, |n| (1..=64).contains(&n));
        mib as u64 * 1024 * 1024
    }
    /// `shared_folders`: a JSON list of id, name and path. A list that isn't
    /// that, or repeats or misspells an id, is reported and none is shared.
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
    /// `webhooks` ([`crate::webhook::parse_webhooks`]). A list it refuses is
    /// reported without its text, which holds the secrets, and no hook is used.
    pub fn webhooks(&self) -> Vec<crate::webhook::Webhook> {
        crate::webhook::parse_webhooks(self.get("webhooks", "[]")).unwrap_or_else(|error| {
            // The outermost message only: it names the URL, never a secret.
            let error = error.to_string();
            if first_report("webhooks", &error) {
                tracing::warn!(
                    key = "webhooks",
                    error,
                    "unsupported setting value; using the default"
                );
            }
            Vec::new()
        })
    }
    /// Pairing by QR code instead of a PIN (off by default).
    pub fn qr_pairing(&self) -> bool {
        self.boolean("qr_pairing", false)
    }
    /// `dd_rotation_option`: `disabled` (the default), `auto` or `manual`.
    pub fn dd_rotation_option(&self) -> DisplayAdjust {
        self.display_adjust("dd_rotation_option")
    }
    /// `dd_manual_rotation`: 0 (the default), 90, 180 or 270 degrees clockwise.
    pub fn dd_manual_rotation(&self) -> u16 {
        self.integer30("dd_manual_rotation", 0, |n| matches!(n, 0 | 90 | 180 | 270)) as u16
    }
    /// `dd_scale_option`: `disabled` (the default), `auto` or `manual`.
    pub fn dd_scale_option(&self) -> DisplayAdjust {
        self.display_adjust("dd_scale_option")
    }
    /// `dd_manual_scale`: 100 to 500 percent in steps of 25, 100 by default.
    pub fn dd_manual_scale(&self) -> u16 {
        self.integer30("dd_manual_scale", 100, |n| {
            (100..=500).contains(&n) && n % 25 == 0
        }) as u16
    }
    fn display_adjust(&self, key: &str) -> DisplayAdjust {
        self.choice30(
            key,
            &[
                ("disabled", DisplayAdjust::Disabled),
                ("auto", DisplayAdjust::Auto),
                ("manual", DisplayAdjust::Manual),
            ],
        )
    }
    /// `background_mode`: `auto` (the default), `child_session` or `virtual_display`.
    pub fn background_mode(&self) -> BackgroundMode {
        self.choice30(
            "background_mode",
            &[
                ("auto", BackgroundMode::Auto),
                ("child_session", BackgroundMode::ChildSession),
                ("virtual_display", BackgroundMode::VirtualDisplay),
            ],
        )
    }
    /// The child session's resolution, `WIDTHxHEIGHT` (1920x1080 by default).
    pub fn child_session_resolution(&self) -> (u32, u32) {
        let value = self.text30("child_session_resolution", "1920x1080");
        crate::display_policy::parse_resolution(value).unwrap_or_else(|| {
            invalid("child_session_resolution", value);
            (1920, 1080)
        })
    }
    /// The child session's scaling in percent (100 to 500, 100 by default).
    pub fn child_session_scale(&self) -> u16 {
        self.integer30("child_session_scale", 100, |n| (100..=500).contains(&n)) as u16
    }
    /// The child session's base port, as `port` is for the host
    /// ([`super::Ports::from_base`]); 48289 by default, so its ports are 48284 to 48310.
    pub fn child_session_port(&self) -> u16 {
        self.integer30("child_session_port", 48289, |n| PORTS.contains(&n)) as u16
    }
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
    }
    Ok(folders)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys a device or an app may override.
    const PER_DEVICE: &[&str] = &[
        "hdr_dolby_vision",
        "hdr_hlg",
        "audio_surround_714",
        "touch_mode",
        "touch_sensitivity",
        "touch_gestures",
        "client_cursor",
        "dd_rotation_option",
        "dd_manual_rotation",
        "dd_scale_option",
        "dd_manual_scale",
    ];

    fn config(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    /// Every accessor's answer, so two configurations can be compared whole.
    fn answers(c: &Config) -> String {
        format!(
            "{:?}",
            (
                (
                    c.hdr_dynamic_metadata(),
                    c.hdr_dolby_vision(),
                    c.hdr_hlg(),
                    c.audio_surround_714(),
                    c.pen_barrel_roll(),
                ),
                (
                    c.touch_keyboard_digitizer(),
                    c.touch_mode(),
                    c.touch_sensitivity(),
                    c.touch_gestures(),
                    c.client_cursor(),
                    c.clipboard_image(),
                    c.clipboard_image_max_bytes(),
                    c.shared_folders(),
                    c.webhooks(),
                ),
                (
                    c.qr_pairing(),
                    c.dd_rotation_option(),
                    c.dd_manual_rotation(),
                    c.dd_scale_option(),
                    c.dd_manual_scale(),
                    c.background_mode(),
                    c.child_session_resolution(),
                    c.child_session_scale(),
                    c.child_session_port(),
                ),
            )
        )
    }

    #[test]
    fn there_are_21_unique_settings() {
        let keys: std::collections::BTreeSet<_> = SETTINGS_30.iter().map(|(k, _)| k).collect();
        assert_eq!((SETTINGS_30.len(), keys.len()), (21, 21));
    }

    #[test]
    fn the_defaults_are_the_documented_values() {
        let c = Config::default();
        assert!(c.hdr_dynamic_metadata() && c.hdr_dolby_vision() && c.hdr_hlg());
        assert!(c.audio_surround_714());
        assert!(c.pen_barrel_roll() && !c.touch_keyboard_digitizer());
        assert_eq!(c.touch_mode(), TouchMode::Client);
        assert_eq!(c.touch_sensitivity(), 100);
        assert!(c.touch_gestures() && c.client_cursor() && c.clipboard_image());
        assert_eq!(c.clipboard_image_max_bytes(), 16 * 1024 * 1024);
        assert!(c.shared_folders().is_empty() && c.webhooks().is_empty());
        assert!(!c.qr_pairing());
        assert_eq!(c.dd_rotation_option(), DisplayAdjust::Disabled);
        assert_eq!(c.dd_manual_rotation(), 0);
        assert_eq!(c.dd_scale_option(), DisplayAdjust::Disabled);
        assert_eq!(c.dd_manual_scale(), 100);
        assert_eq!(c.background_mode(), BackgroundMode::Auto);
        assert_eq!(c.child_session_resolution(), (1920, 1080));
        assert_eq!(c.child_session_scale(), 100);
        assert_eq!(c.child_session_port(), 48289);
        let ports = super::super::Ports::from_base(c.child_session_port());
        assert_eq!((ports.https, ports.rtsp), (48284, 48310));
    }

    #[test]
    fn each_default_written_out_reads_as_the_unset_default() {
        let unset = answers(&Config::default());
        let written: String = SETTINGS_30
            .iter()
            .map(|(key, default)| format!("{key} = {default}\n"))
            .collect();
        assert_eq!(answers(&config(&written)), unset);
        for (key, default) in SETTINGS_30 {
            assert_eq!(
                answers(&config(&format!("{key} = {default}\n"))),
                unset,
                "{key}"
            );
        }
    }

    #[test]
    fn values_the_host_cannot_use_fall_back_to_the_default() {
        let unset = answers(&Config::default());
        for text in [
            "hdr_hlg = maybe",
            "touch_mode = pen",
            "touch_sensitivity = 24",
            "touch_sensitivity = 401",
            "clipboard_image_max_mb = 0",
            "clipboard_image_max_mb = 65",
            "shared_folders = {\"id\": \"games\"}",
            "shared_folders = [{\"id\": \"../games\", \"name\": \"Games\", \"path\": \"D:\\\\Games\"}]",
            "shared_folders = [{\"id\": \"games\", \"name\": \"Games\"}]",
            "shared_folders = [{\"id\": \"games\", \"name\": \"\", \"path\": \"D:\\\\Games\"}]",
            "shared_folders = [{\"id\": \"games\", \"name\": \"Games\", \"path\": \"D:\\\\Games\", \"mode\": \"rw\"}]",
            "webhooks = [{\"url\": \"ftp://x\", \"events\": [\"app.started\"], \"secret\": \"s\"}]",
            "webhooks = not json",
            "dd_rotation_option = sideways",
            "dd_manual_rotation = 45",
            "dd_manual_rotation = 360",
            "dd_scale_option = big",
            "dd_manual_scale = 110",
            "dd_manual_scale = 75",
            "dd_manual_scale = 525",
            "background_mode = session",
            "child_session_resolution = 1920",
            "child_session_resolution = 0x1080",
            "child_session_scale = 99",
            "child_session_scale = 501",
            "child_session_port = 1028",
            "child_session_port = 65515",
        ] {
            assert_eq!(answers(&config(text)), unset, "{text}");
        }
    }

    #[test]
    fn values_in_range_are_read() {
        let c = config(concat!(
            "touch_mode = trackpad\n",
            "touch_sensitivity = 25\n",
            "clipboard_image_max_mb = 64\n",
            "dd_rotation_option = manual\n",
            "dd_manual_rotation = 270\n",
            "dd_scale_option = auto\n",
            "dd_manual_scale = 125\n",
            "background_mode = child_session\n",
            "child_session_resolution = 2560x1440\n",
            "child_session_scale = 150\n",
            "child_session_port = 50000\n",
        ));
        assert_eq!(c.touch_mode(), TouchMode::Trackpad);
        assert_eq!(c.touch_sensitivity(), 25);
        assert_eq!(c.clipboard_image_max_bytes(), 64 * 1024 * 1024);
        assert_eq!(c.dd_rotation_option(), DisplayAdjust::Manual);
        assert_eq!(c.dd_manual_rotation(), 270);
        assert_eq!(c.dd_scale_option(), DisplayAdjust::Auto);
        assert_eq!(c.dd_manual_scale(), 125);
        assert_eq!(c.background_mode(), BackgroundMode::ChildSession);
        assert_eq!(c.child_session_resolution(), (2560, 1440));
        assert_eq!(c.child_session_scale(), 150);
        assert_eq!(c.child_session_port(), 50000);
        let folders = config(
            "shared_folders = [{\"id\": \"games\", \"name\": \"Games\", \"path\": \"D:\\\\Games\"}, {\"id\": \"saves-2\", \"name\": \"Saves\", \"path\": \"E:\\\\Saves\"}]",
        )
        .shared_folders();
        assert_eq!(
            folders
                .iter()
                .map(|f| (f.id.as_str(), f.path.to_str().unwrap()))
                .collect::<Vec<_>>(),
            [("games", "D:\\Games"), ("saves-2", "E:\\Saves")]
        );
        let twice = "[{\"id\": \"a\", \"name\": \"A\", \"path\": \"C:\\\\A\"}, {\"id\": \"a\", \"name\": \"B\", \"path\": \"C:\\\\B\"}]";
        assert!(
            config(&format!("shared_folders = {twice}"))
                .shared_folders()
                .is_empty()
        );
        let hooks = config(
            "webhooks = [{\"url\": \"https://example.test/hook\", \"events\": [\"app.started\"], \"secret\": \"s\"}]",
        )
        .webhooks();
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn per_device_settings_are_exactly_the_documented_ones() {
        for (key, _) in SETTINGS_30 {
            assert_eq!(
                crate::config::override_allowed(key),
                PER_DEVICE.contains(key),
                "{key}"
            );
        }
        assert_eq!(PER_DEVICE.len(), 11);
    }

    /// The settings whose features have shipped, so the console shows them.
    const LIVE: &[&str] = &["audio_surround_714"];

    /// Each setting's key and default in extensions.ts, with LF or CRLF line
    /// ends (CI checks the repository out with CRLF).
    fn schema_settings(schema: &str) -> Vec<(&str, &str)> {
        let mut listed = Vec::new();
        for entry in schema.split("key: '").skip(1) {
            let key = &entry[..entry.find('\'').unwrap()];
            let default = entry
                .split("\n    default: ")
                .nth(1)
                .unwrap_or_else(|| panic!("{key} has no default"));
            let default = default
                .lines()
                .next()
                .and_then(|line| line.strip_suffix(','))
                .unwrap_or_else(|| panic!("{key}'s default does not end its line with a comma"))
                .trim_matches('\'');
            assert_eq!(
                entry.contains("\n    pending: true,"),
                !LIVE.contains(&key),
                "{key} is pending only until its feature ships"
            );
            listed.push((key, default));
        }
        listed.sort();
        listed
    }

    #[test]
    fn the_console_schema_lists_exactly_these_settings_with_these_defaults() {
        let schema = include_str!("../../../web/src/lib/schema/extensions.ts");
        let mut documented = SETTINGS_30.to_vec();
        documented.sort();
        assert_eq!(schema_settings(schema), documented);
        let crlf = schema.replace("\r\n", "\n").replace('\n', "\r\n");
        assert_eq!(schema_settings(&crlf), documented);
    }
}
