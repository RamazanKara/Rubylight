//! Where the host keeps its profile on Windows.
use anyhow::{Context, Result};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Rubylight's machine-wide folder, `%ProgramData%\Butterpollo`: the
/// installed service's profile and what every profile on this PC shares.
/// Falls back to `C:\ProgramData` when the variable is unset, as it is for
/// some service accounts.
pub fn data_root() -> PathBuf {
    PathBuf::from(std::env::var_os("PROGRAMDATA").unwrap_or_else(|| r"C:\ProgramData".into()))
        .join("Butterpollo")
}

/// The installed service's profile, `%ProgramData%\Butterpollo\config`.
pub fn installed_profile() -> PathBuf {
    data_root().join("config")
}

/// A portable host uses an existing `config` folder beside its executable,
/// or `%LocalAppData%\ButterpolloRust\config` otherwise.
pub fn portable_profile() -> Result<PathBuf> {
    portable_profile_at(
        &std::env::current_exe()?,
        std::env::var_os("LOCALAPPDATA").as_deref(),
    )
}

fn portable_profile_at(executable: &Path, local_app_data: Option<&OsStr>) -> Result<PathBuf> {
    let directory = executable
        .parent()
        .context("package directory unavailable")?
        .join("config");
    if directory.is_dir() {
        return Ok(directory);
    }
    Ok(
        PathBuf::from(local_app_data.context("LOCALAPPDATA is unavailable")?)
            .join("ButterpolloRust/config"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_packages_use_separate_adjacent_profiles() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("systemprofile");
        for name in ["seat-one", "seat-two"] {
            let package = root.path().join(name);
            let profile = package.join("config");
            std::fs::create_dir_all(&profile).unwrap();
            for binary in ["butterpollo.exe", "Start Rubylight.exe"] {
                assert_eq!(
                    portable_profile_at(&package.join(binary), Some(local.as_os_str())).unwrap(),
                    profile
                );
                assert_eq!(
                    portable_profile_at(&package.join(binary), None).unwrap(),
                    profile
                );
            }
        }
    }

    #[test]
    fn without_an_adjacent_directory_keeps_the_per_user_profile() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("butterpollo.exe");
        let local = root.path().join("local");
        let expected = local.join("ButterpolloRust/config");
        assert_eq!(
            portable_profile_at(&executable, Some(local.as_os_str())).unwrap(),
            expected
        );
        std::fs::write(root.path().join("config"), "not a directory").unwrap();
        assert_eq!(
            portable_profile_at(&executable, Some(local.as_os_str())).unwrap(),
            expected
        );
        assert!(portable_profile_at(&executable, None).is_err());
    }
}
