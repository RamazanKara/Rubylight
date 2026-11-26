//! Per-PC/per-game setting overrides, named profiles, and untrusted scan-sync displays.

use serde::{Deserialize, Serialize};

pub use nightjar_policy::display_profile::DisplayMatcher;

use super::labels;
use super::migrate::{fold_lower_bitrate, fold_tell_host, normalize_values};
use super::Settings;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Overrides(pub(super) serde_json::Map<String, serde_json::Value>);

impl<'de> Deserialize<'de> for Overrides {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut values = serde_json::Map::deserialize(deserializer)?;
        serde_json::from_value::<Settings>(serde_json::Value::Object(values.clone()))
            .map_err(serde::de::Error::custom)?;
        normalize_values(&mut values);
        fold_lower_bitrate(&mut values);
        fold_tell_host(&mut values);
        Ok(Self(values))
    }
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn has(&self, keys: &[&str]) -> bool {
        keys.iter().any(|key| self.0.contains_key(*key))
    }

    pub fn capture(&mut self, settings: &Settings, keys: &[&str]) {
        for key in keys {
            self.0.insert(
                (*key).to_owned(),
                super::override_fields::capture_field(settings, key),
            );
        }
    }

    pub fn clear(&mut self, keys: &[&str]) {
        for key in keys {
            self.0.remove(*key);
        }
    }

    pub fn apply(&self, base: &Settings) -> Settings {
        if self.0.is_empty() {
            return base.clone();
        }
        let mut applied = base.clone();
        for (key, value) in &self.0 {
            if let Err(e) = super::override_fields::apply_field(&mut applied, key, value) {
                // A value this version cannot read is left out with the base's own, not a panic
                // on the UI thread: a file from a newer Nightjar, or a hand edit, costs its entry.
                eprintln!("nightjar: ignoring settings overrides this version cannot read: {e}");
                return base.clone();
            }
        }
        // Resolution spans `width`, `height`, and `native_scale`: pre-6.4.1 layers that set pixel
        // dimensions without `native_scale` must keep full 100% scale rather than inheriting main's.
        if (self.0.contains_key("width") || self.0.contains_key("height"))
            && !self.0.contains_key("native_scale")
        {
            applied.native_scale = 100;
        }
        applied
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
    /// The resolution scale in percent this profile keeps, plain data for the stream settings'
    /// scale control; None (every profile saved before 7.0) leaves the resolution to `settings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_scale_pct: Option<u8>,
    /// The screen this profile is kept for: a stream started there picks it (`Profiles::selected`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_matcher: Option<DisplayMatcher>,
}

impl Profile {
    pub fn new(id: u64, name: impl Into<String>, settings: Overrides) -> Self {
        Self { id, name: name.into(), settings, resolution_scale_pct: None, display_matcher: None }
    }
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
    /// The screen Nightjar's window is on now, set by the app as the window moves; never saved.
    /// None (not known yet) leaves the choice to the network and the profile in use.
    #[serde(skip)]
    pub screen: Option<DisplayMatcher>,
}

impl Profiles {
    pub fn selected(&self, network: crate::network_types::Kind) -> Option<u64> {
        if let Some(id) = self.for_screen() {
            return Some(id);
        }
        let rule = if self.automatic {
            match network {
                crate::network_types::Kind::Mobile => self.mobile,
                crate::network_types::Kind::HomeWifi => self.home,
                crate::network_types::Kind::Wifi => self.wifi,
                crate::network_types::Kind::Other => None,
            }
        } else {
            None
        };
        rule.or(self.active)
    }

    /// The profile kept for the screen the window is on now, which a stream there uses ahead of
    /// the network choice and the profile in use.
    pub fn for_screen(&self) -> Option<u64> {
        nightjar_policy::display_profile::pick(
            self.entries.iter().map(|p| (p.id, p.display_matcher.as_ref())),
            self.screen.as_ref(),
        )
    }

    /// Keeps profile `id` for `screen`, or for no screen with None. A screen calls up one profile,
    /// so another profile kept for the same screen lets it go.
    pub fn keep_for_screen(&mut self, id: u64, screen: Option<&DisplayMatcher>) {
        for profile in &mut self.entries {
            if profile.id == id {
                profile.display_matcher = screen.cloned();
            } else if let (Some(screen), Some(kept)) = (screen, &profile.display_matcher)
                && kept.matches(screen)
            {
                profile.display_matcher = None;
            }
        }
    }

    pub fn settings(&self, base: &Settings, network: crate::network_types::Kind) -> Settings {
        self.profile(network)
            .map_or_else(|| base.clone(), |p| p.settings.apply(base))
    }

    /// The profile a stream on `network` uses, None for the main settings.
    pub fn profile(&self, network: crate::network_types::Kind) -> Option<&Profile> {
        self.entries.iter().find(|p| Some(p.id) == self.selected(network))
    }

    pub fn bitrate_limit(&self, network: crate::network_types::Kind) -> u32 {
        if network == crate::network_types::Kind::Mobile {
            self.mobile_limit_kbps
        } else {
            0
        }
    }

    pub fn check_name(&self, name: &str, except: Option<u64>) -> Result<(), &'static str> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 32 {
            return Err(labels::PROFILE_NAME_LEN);
        }
        if self
            .entries
            .iter()
            .any(|p| Some(p.id) != except && p.name.to_lowercase() == name.to_lowercase())
        {
            return Err(labels::PROFILE_NAME_TAKEN);
        }
        Ok(())
    }

    pub fn create(&mut self, name: &str, settings: Overrides) -> Result<u64, &'static str> {
        self.check_name(name, None)?;
        if self.entries.len() >= 50 {
            return Err(labels::PROFILE_LIMIT);
        }
        let id = self
            .entries
            .iter()
            .map(|p| p.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(labels::PROFILE_NO_IDS)?;
        self.entries.push(Profile::new(id, name.trim(), settings));
        Ok(id)
    }

    pub fn delete(&mut self, id: u64) {
        self.entries.retain(|p| p.id != id);
        if self.active == Some(id) {
            self.active = None;
        }
        for rule in [&mut self.mobile, &mut self.home, &mut self.wifi] {
            if *rule == Some(id) {
                *rule = None;
            }
        }
    }
}

/// The displays whose presents a stream found not starting their scans (`stream::scan_sync`),
/// by the monitor's device interface path (`\\?\DISPLAY#...#{...}`, its hardware ID and instance,
/// rather than the output name `\.\DISPLAY1` that another monitor can take over), kept in
/// `scan_untrusted.json`: Whole frames under VRR Automatic starts on Windows V-Sync on them from
/// the next run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanUntrusted {
    pub displays: Vec<String>,
}
