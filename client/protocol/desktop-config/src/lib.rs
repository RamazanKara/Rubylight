//! Nightjar desktop configuration, localized UI strings, keyboard/gamepad bindings,
//! PIN lock policy and pure state/math types shared across the desktop crates without
//! pulling `egui`, `sdl3-sys`, `windows`, `ring` or native media SDKs.

pub const VERSION: &str = env!("NIGHTJAR_VERSION");

pub mod app_types;
pub mod config;
pub mod gamepad_types;
pub mod hosts_types;
pub mod latency_bench_state;
pub mod network_types;
pub mod pads_view;
pub mod pin_lock;
pub mod power_types;
pub mod shortcuts;
pub mod stream_types;
pub mod strings;
mod strings_70;
pub mod version_name;
pub mod window_place_types;
pub mod wol_types;

use std::path::{Path, PathBuf};

/// The platform Downloads folder.
pub fn downloads_dir() -> Option<PathBuf> {
    directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
        .or_else(platform_downloads_fallback)
}

fn platform_downloads_fallback() -> Option<PathBuf> {
    // Linux and macOS share the XDG_DOWNLOAD_DIR and $HOME/Downloads fallback.
    #[cfg(unix)]
    {
        unix_downloads_fallback(
            std::env::var_os("XDG_DOWNLOAD_DIR").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
    }
    // Windows relies on SHGetKnownFolderPath via directories::UserDirs rather than Unix env vars.
    #[cfg(not(unix))]
    {
        None
    }
}

// Compiled on Unix (Linux and macOS) runtime targets and in cross-platform unit tests.
#[cfg(any(unix, test))]
fn unix_downloads_fallback(
    xdg_download: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    if let Some(dir) = xdg_download.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    home.filter(|v| !v.is_empty())
        .map(|dir| PathBuf::from(dir).join("Downloads"))
}

static INTERNET_HOOK: std::sync::OnceLock<fn(bool)> = std::sync::OnceLock::new();

/// Registers the tunnel toggle invoked whenever settings are loaded or saved.
pub fn register_internet_hook(hook: fn(bool)) {
    let _ = INTERNET_HOOK.set(hook);
}

pub(crate) fn notify_internet_enabled(on: bool) {
    if let Some(hook) = INTERNET_HOOK.get() {
        hook(on);
    }
}

impl From<config::HostDisplay> for nightjar_policy::http::HostDisplay {
    fn from(value: config::HostDisplay) -> Self {
        match value {
            config::HostDisplay::HostDefault => Self::HostDefault,
            config::HostDisplay::Exclusive => Self::Exclusive,
            config::HostDisplay::ExtendedPrimary => Self::ExtendedPrimary,
            config::HostDisplay::Extended => Self::Extended,
            config::HostDisplay::ExtendedIsolated => Self::ExtendedIsolated,
            config::HostDisplay::ExtendedPrimaryIsolated => Self::ExtendedPrimaryIsolated,
            config::HostDisplay::Physical => Self::Physical,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::unix_downloads_fallback;

    #[test]
    fn unix_downloads_fallback_uses_xdg_or_home_downloads() {
        assert_eq!(
            unix_downloads_fallback(Some(OsStr::new("/tmp/dl")), Some(OsStr::new("/home/u"))),
            Some(PathBuf::from("/tmp/dl"))
        );
        assert_eq!(
            unix_downloads_fallback(Some(OsStr::new("")), Some(OsStr::new("/home/u"))),
            Some(PathBuf::from("/home/u").join("Downloads"))
        );
        assert_eq!(
            unix_downloads_fallback(None, Some(OsStr::new("/home/u"))),
            Some(PathBuf::from("/home/u").join("Downloads"))
        );
        assert_eq!(unix_downloads_fallback(None, Some(OsStr::new(""))), None);
        assert_eq!(unix_downloads_fallback(None, None), None);
    }
}
