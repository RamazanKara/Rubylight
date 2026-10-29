//! Where Steam is installed on this PC.

use butterpollo_core::nucleus::generic;
use std::path::{Path, PathBuf};
use windows::{
    Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    },
    core::PCWSTR,
};

/// A string registry value; an empty `value` reads the key's default value.
pub(crate) fn registry_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let value: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: `key` and `value` are NUL-terminated and outlive the call, and `size` is the byte
    // size of `buffer`.
    unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    let length = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length])).filter(|s| !s.is_empty())
}
/// Steam installations: the installer's registry entries, then the usual
/// folders. The host runs as SYSTEM, so the signed-in user's folders come
/// from that user's environment, and a folder on a network share is skipped.
pub fn roots() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = [
        (
            HKEY_LOCAL_MACHINE,
            "SOFTWARE\\WOW6432Node\\Valve\\Steam",
            "InstallPath",
        ),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\Valve\\Steam", "InstallPath"),
        (HKEY_CURRENT_USER, "Software\\Valve\\Steam", "SteamPath"),
    ]
    .into_iter()
    .filter_map(|(root, key, value)| registry_string(root, key, value))
    .map(PathBuf::from)
    .collect();
    let user = crate::process::user_environment().unwrap_or_default();
    for variable in ["PROGRAMFILES(X86)", "PROGRAMFILES"] {
        if let Some(folder) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(folder).join("Steam"));
        }
    }
    if let Some(local) = user.get("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Steam"));
    }
    installations(candidates)
}

/// Whether `path` is on one of this PC's drives. The host runs as SYSTEM:
/// opening a network share would hand it the PC's account.
fn on_this_pc(path: &Path) -> bool {
    generic::on_this_pc(&path.to_string_lossy()).is_some()
}

/// The `candidates` that are Steam installations on this PC's drives; one on
/// a share is dropped before anything opens it.
fn installations(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![];
    for candidate in candidates.into_iter().filter(|path| on_this_pc(path)) {
        let Ok(path) = std::fs::canonicalize(&candidate) else {
            continue;
        };
        // A link to a share resolves to one.
        if !on_this_pc(&path) {
            continue;
        }
        // %LOCALAPPDATA%\Steam holds only Steam's web cache.
        let installation = path.join("steamapps").is_dir() || path.join("steam.exe").is_file();
        if installation && !roots.contains(&path) {
            roots.push(path);
        }
    }
    // canonicalize gives \\?\ paths; keep the form Steam and users write.
    roots
        .into_iter()
        .map(|path| {
            let text = path.to_string_lossy();
            PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_steam_folder_on_a_network_share_is_never_opened() {
        let local = tempfile::tempdir().unwrap();
        std::fs::write(local.path().join("steam.exe"), b"").unwrap();
        let reached = tempfile::tempdir().unwrap();
        std::fs::write(reached.path().join("steam.exe"), b"").unwrap();
        // A share path that reaches a real installation: dropping it is what
        // keeps it out.
        let globalroot = PathBuf::from(format!(
            r"\\?\GLOBALROOT\GLOBAL??\{}",
            reached.path().display()
        ));
        assert!(globalroot.join("steam.exe").is_file());
        let found = installations(vec![
            PathBuf::from(r"\\nas\Steam"),
            PathBuf::from(r"\\?\UNC\nas\Steam"),
            PathBuf::from(r"\\.\UNC\nas\Steam"),
            globalroot,
            local.path().to_path_buf(),
        ]);
        let canonical = std::fs::canonicalize(local.path()).unwrap();
        let text = canonical.to_string_lossy();
        assert_eq!(
            found,
            [PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))]
        );
    }
}
