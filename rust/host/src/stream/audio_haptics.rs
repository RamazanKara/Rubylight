//! Haptics from game audio on the control loop (`butterpollo_core::audio_haptics`):
//! the settings follow saves mid-stream, and a value the host would not use
//! says so and keeps them off.
use anyhow::Result;
use butterpollo_core::audio_haptics::{Arbiter, Settings};
use butterpollo_core::config::Config;
use butterpollo_core::session::Warnings;

/// The session warning code while a setting is refused.
pub(super) const WARNING: &str = "audio_haptics";

/// One control peer's audio haptics and the configuration version they follow.
#[derive(Default)]
pub(super) struct PeerHaptics {
    pub(super) arbiter: Arbiter,
    version: Option<u64>,
}

impl PeerHaptics {
    /// Reads the settings again when the host configuration `version` moved.
    pub(super) fn follow(
        &mut self,
        version: u64,
        warnings: &Warnings,
        config: impl FnOnce() -> Result<Config>,
    ) {
        if self.version == Some(version) {
            return;
        }
        self.version = Some(version);
        let settings = match config().and_then(|config| Settings::from_config(&config)) {
            Ok(settings) => {
                warnings.clear(WARNING);
                settings
            }
            Err(error) => {
                warnings.set(
                    WARNING,
                    format!(
                        "Haptics from game audio are off ({error:#}). Correct the setting under Controllers."
                    ),
                );
                Settings::default()
            }
        };
        if settings != self.arbiter.settings() {
            tracing::info!(
                enabled = settings.enabled,
                strength = settings.strength,
                curve = ?settings.curve,
                gate_db = settings.gate_db,
                "haptics from game audio"
            );
            self.arbiter.set_settings(settings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::audio_haptics::{Curve, STRENGTH};

    fn warned(warnings: &Warnings) -> bool {
        warnings.snapshot().iter().any(|w| w.code == WARNING)
    }

    fn config(values: &[(&str, &str)]) -> Result<Config> {
        Ok(Config {
            values: values
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        })
    }

    #[test]
    fn a_save_applies_mid_stream_and_a_refused_value_turns_them_off_loudly() {
        let warnings = Warnings::default();
        let mut peer = PeerHaptics::default();
        peer.follow(1, &warnings, || {
            config(&[("audio_haptics", "on"), ("audio_haptics_curve", "soft")])
        });
        assert!(peer.arbiter.settings().enabled);
        assert_eq!(peer.arbiter.settings().curve, Curve::Soft);
        // The same version is not read again.
        peer.follow(1, &warnings, || panic!("read again"));
        peer.follow(2, &warnings, || {
            config(&[("audio_haptics", "on"), (STRENGTH, "40")])
        });
        assert_eq!(peer.arbiter.settings().strength, 40);
        assert!(!warned(&warnings));
        peer.follow(3, &warnings, || {
            config(&[("audio_haptics", "on"), (STRENGTH, "900")])
        });
        assert!(!peer.arbiter.settings().enabled);
        assert!(warned(&warnings));
        peer.follow(4, &warnings, || anyhow::bail!("config unreadable"));
        assert!(!peer.arbiter.settings().enabled);
        peer.follow(5, &warnings, || config(&[("audio_haptics", "on")]));
        assert!(peer.arbiter.settings().enabled);
        assert!(!warned(&warnings));
    }
}
