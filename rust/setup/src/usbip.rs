//! Persist the optional shared USB/IP driver choice across setup and updates.
use anyhow::{Context, Result};
use std::path::Path;

const CHOICE: &str = "usbip-driver.json";

pub fn selected(profile: &Path, explicit: Option<bool>) -> bool {
    explicit.unwrap_or_else(|| {
        std::fs::read(profile.join(CHOICE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|choice| choice["enabled"] == true)
    })
}

pub fn remember(profile: &Path, enabled: bool) -> Result<()> {
    std::fs::create_dir_all(profile)?;
    std::fs::write(
        profile.join(CHOICE),
        if enabled {
            br#"{"enabled":true}"#.as_slice()
        } else {
            br#"{"enabled":false}"#.as_slice()
        },
    )
    .context("saving the USB audio and haptics installer choice")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usbip_stays_optional_and_updates_keep_the_explicit_choice() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let profile = temp.path();
        assert!(!selected(profile, None));
        assert!(selected(profile, Some(true)));
        remember(profile, true)?;
        assert!(selected(profile, None));
        assert!(!selected(profile, Some(false)));
        remember(profile, false)?;
        assert!(!selected(profile, None));
        std::fs::write(profile.join(CHOICE), b"interrupted write")?;
        assert!(!selected(profile, None));
        Ok(())
    }
}
