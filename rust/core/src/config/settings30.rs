//! The settings of the 3.0 extensions, defined ahead of their features: the
//! console lists them in `web/src/lib/schema/extensions.ts`, hidden until
//! each workstream ships its feature. A value the
//! host can't use is reported through [`invalid`] and the default applies,
//! so a launch never fails because of one of these settings.
use super::{Config, first_report, invalid, unquote};
use std::path::PathBuf;

/// Every 3.0 setting with its default, as `sunshine.conf` would spell it.
pub const SETTINGS_30: &[(&str, &str)] = &[
    ("audio_surround_714", "true"),
    ("pen_barrel_roll", "true"),
    ("touch_keyboard_digitizer", "false"),
    ("touch_mode", "client"),
    ("touch_sensitivity", "100"),
    ("touch_gestures", "true"),
    ("client_cursor", "true"),
    ("webhooks", "[]"),
    ("qr_pairing", "true"),
    ("dd_rotation_option", "disabled"),
    ("dd_manual_rotation", "0"),
    // Nucleus Co-op background streaming and co-op.
    ("nucleus_path", ""),
    ("nucleus_coop_players", "4"),
];

/// `touch_mode`: how touches reach the PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchMode {
    /// As the device sends them; `native_pen_touch` still applies.
    Client,
    Mouse,
    Trackpad,
}

/// `dd_rotation_option`, beside `dd_resolution_option`. Windows scaling has no
/// option of its own: `dd_virtual_display_scale` sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayAdjust {
    /// Leave the display as it is.
    Disabled,
    /// Match the device.
    Auto,
    /// Use the `dd_manual_*` value.
    Manual,
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
    /// `webhooks` ([`crate::webhook::parse_webhooks`]). A list it refuses is
    /// reported without its text, which holds the secrets, and no hook is used.
    pub fn webhooks(&self) -> Vec<crate::webhook::Webhook> {
        crate::webhook::parse_webhooks(self.get("webhooks", "[]")).unwrap_or_else(|error| {
            // The outermost message only: it names a hook by its origin, never its secret.
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
        self.boolean("qr_pairing", true)
    }
    /// `dd_rotation_option`: `disabled` (the default), `auto` or `manual`.
    pub fn dd_rotation_option(&self) -> DisplayAdjust {
        self.display_adjust("dd_rotation_option")
    }
    /// `dd_manual_rotation`: 0 (the default), 90, 180 or 270 degrees clockwise.
    pub fn dd_manual_rotation(&self) -> u16 {
        self.integer30("dd_manual_rotation", 0, |n| matches!(n, 0 | 90 | 180 | 270)) as u16
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
    /// The Nucleus Co-op folder: `nucleus_path`, or None for the host's own
    /// copy, which it installs on first use.
    pub fn nucleus_path(&self) -> Option<PathBuf> {
        let value = self.text30("nucleus_path", "");
        (!value.is_empty()).then(|| PathBuf::from(value))
    }
    /// The most players a co-op session gets (1 to 4, 4 by default); the
    /// game's handler and the free controller slots can allow fewer.
    pub fn nucleus_coop_players(&self) -> u32 {
        self.integer30("nucleus_coop_players", 4, |n| (1..=4).contains(&n)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys a device or an app may override.
    const PER_DEVICE: &[&str] = &[
        "audio_surround_714",
        "touch_mode",
        "touch_sensitivity",
        "touch_gestures",
        "client_cursor",
        "dd_rotation_option",
        "dd_manual_rotation",
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
                ),
                (c.nucleus_path(), c.nucleus_coop_players()),
            )
        )
    }

    #[test]
    fn there_are_13_unique_settings() {
        let keys: std::collections::BTreeSet<_> = SETTINGS_30.iter().map(|(k, _)| k).collect();
        assert_eq!((SETTINGS_30.len(), keys.len()), (13, 13));
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
        assert!(c.qr_pairing());
        assert_eq!(c.dd_rotation_option(), DisplayAdjust::Disabled);
        assert_eq!(c.dd_manual_rotation(), 0);
        assert_eq!(c.nucleus_path(), None);
        assert_eq!(c.nucleus_coop_players(), 4);
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
            "nucleus_coop_players = 0",
            "nucleus_coop_players = 5",
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
            "nucleus_path = \"D:\\Tools\\Nucleus\"\n",
            "nucleus_coop_players = 2\n",
        ));
        assert_eq!(c.touch_mode(), TouchMode::Trackpad);
        assert_eq!(c.touch_sensitivity(), 25);
        assert_eq!(c.clipboard_image_max_bytes(), 64 * 1024 * 1024);
        assert_eq!(c.dd_rotation_option(), DisplayAdjust::Manual);
        assert_eq!(c.dd_manual_rotation(), 270);
        assert_eq!(c.nucleus_path(), Some(PathBuf::from("D:\\Tools\\Nucleus")));
        assert_eq!(c.nucleus_coop_players(), 2);
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
        assert_eq!(PER_DEVICE.len(), 7);
    }

    /// The settings whose features have shipped, so the console shows them.
    const LIVE: &[&str] = &[
        "audio_surround_714",
        "dd_rotation_option",
        "dd_manual_rotation",
        // Input: pen barrel roll, the device-drawn cursor and touch profiles.
        "pen_barrel_roll",
        "client_cursor",
        "touch_mode",
        "touch_sensitivity",
        "touch_gestures",
        // Nucleus Co-op.
        "nucleus_path",
        "nucleus_coop_players",
    ];

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
