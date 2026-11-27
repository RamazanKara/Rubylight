//! Cross-domain validation hooks for `Config::update` and `Config::check_saved`.

use anyhow::{Result, bail};
use butterpollo_config::config::Config;
use butterpollo_input_policy::{audio_haptics, couch_menu, dualsense, input_policy};
use butterpollo_media_policy::{framegen, stream_preset};

/// Full cross-domain `Config` validation and persistence checks.
pub trait ConfigExt {
    fn update(&mut self, object: &serde_json::Map<String, serde_json::Value>) -> Result<()>;
    fn check_saved<'a>(&self, keys: impl IntoIterator<Item = &'a String>) -> Result<()>;
}

fn check_dvr_directory(value: &str) -> Result<()> {
    let path = butterpollo_config::config::unquote(value.trim()).trim();
    if let Some(refusal) = crate::network_path::refused(path) {
        let message = match refusal {
            crate::network_path::Refusal::Network => {
                r"dvr_directory cannot be a network path (\\server\share): the host would sign in to that computer with this PC's account. Pick a folder on this PC or a mapped drive letter."
            }
            crate::network_path::Refusal::Device => {
                r"dvr_directory cannot be a device path (\\.\ or \\?\, NUL, COM1 and the like). Pick a folder on this PC."
            }
        };
        bail!("{message}");
    }
    Ok(())
}

impl ConfigExt for Config {
    fn update(&mut self, object: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
        self.update_with(object, |next, changed| {
            if changed("dvr_directory") {
                check_dvr_directory(&next.values["dvr_directory"])?;
            }
            if changed("frame_limiter_fps_limit") {
                framegen::Rate::parse(next.get("frame_limiter_fps_limit", "0"))?;
            }
            if changed(stream_preset::SETTING)
                && !stream_preset::valid(next.get(stream_preset::SETTING, ""))
            {
                bail!("stream_preset must be lowest_latency, balanced or reference");
            }
            if changed(crate::nucleus::mode::KEY) {
                crate::nucleus::mode::parse(&next.values[crate::nucleus::mode::KEY])
                    .map_err(anyhow::Error::msg)?;
            }
            if changed(dualsense::HAPTICS_GAIN) {
                dualsense::check_haptics_gain(&next.values[dualsense::HAPTICS_GAIN])?;
            }
            for key in audio_haptics::SETTINGS {
                if changed(key) {
                    audio_haptics::check(key, &next.values[key])?;
                }
            }
            for key in couch_menu::chord::SETTINGS {
                if changed(key)
                    && let Some(warning) = couch_menu::chord::check(key, &next.values[key])
                        .map_err(anyhow::Error::msg)?
                {
                    tracing::warn!("{warning}");
                }
            }
            // Warn, not refuse: either setting alone is fine. The couch menu
            // values were checked above, so the read refuses none.
            if couch_menu::chord::SETTINGS
                .into_iter()
                .chain(["back_button_timeout"])
                .any(changed)
                && let Some(warning) = couch_menu::chord::OpenWith::read(next, &mut |_, _, _| {})
                    .select_conflict(input_policy::back_button_timeout(next))
            {
                tracing::warn!("{warning}");
            }
            Ok(())
        })
    }

    fn check_saved<'a>(&self, keys: impl IntoIterator<Item = &'a String>) -> Result<()> {
        self.check_saved_with(keys, |key, value| {
            if key == "dvr_directory" {
                check_dvr_directory(value)?;
            }
            if key == crate::steamgriddb::KEY_SETTING {
                crate::steamgriddb::check_key(value).map_err(|message| anyhow::anyhow!(message))?;
            }
            Ok(())
        })
    }
}
