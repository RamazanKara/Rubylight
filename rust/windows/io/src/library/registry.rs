//! Windows registry helpers for local launcher discovery.
use crate::steam::registry_string;
use anyhow::{Context, Result};
use std::path::PathBuf;
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, RegCloseKey,
            RegEnumKeyExW, RegOpenKeyExW,
        },
    },
    core::{PCWSTR, PWSTR},
};

pub(super) struct Installed {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) publisher: String,
    pub(super) folder: PathBuf,
    pub(super) uninstall: String,
}

pub(super) fn subkeys(root: HKEY, path: &str) -> Result<Vec<String>> {
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut key = HKEY::default();
    // SAFETY: the path is terminated; key is an out-pointer. Every successfully
    // opened key is closed after enumeration, including an enumeration error.
    let opened = unsafe { RegOpenKeyExW(root, PCWSTR(wide.as_ptr()), Some(0), KEY_READ, &mut key) };
    if opened == ERROR_FILE_NOT_FOUND || opened == ERROR_PATH_NOT_FOUND {
        return Ok(vec![]);
    }
    opened
        .ok()
        .with_context(|| format!("Reading registry {path}"))?;
    let result = (|| {
        let mut entries = vec![];
        for index in 0.. {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            // SAFETY: the live key and output buffer remain valid for this call.
            let result = unsafe {
                RegEnumKeyExW(
                    key,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if result == ERROR_NO_MORE_ITEMS {
                return Ok(entries);
            }
            result
                .ok()
                .with_context(|| format!("Listing registry {path}"))?;
            entries.push(String::from_utf16_lossy(&name[..length as usize]));
        }
        unreachable!()
    })();
    // SAFETY: key was opened above and has not been closed.
    let _ = unsafe { RegCloseKey(key) };
    result
}

pub(super) fn locations(key: &str) -> Vec<(HKEY, String)> {
    let mut roots = vec![
        (HKEY_LOCAL_MACHINE, key.into()),
        (
            HKEY_LOCAL_MACHINE,
            key.replacen("SOFTWARE\\", "SOFTWARE\\WOW6432Node\\", 1),
        ),
    ];
    if let Some(sid) = crate::process::user_sid() {
        roots.push((HKEY_USERS, format!("{sid}\\{key}")));
        roots.push((
            HKEY_USERS,
            format!(
                "{sid}\\{}",
                key.replacen("SOFTWARE\\", "SOFTWARE\\WOW6432Node\\", 1)
            ),
        ));
    } else {
        roots.push((HKEY_CURRENT_USER, key.into()));
    }
    roots
}

pub(super) fn records(key: &str, errors: &mut Vec<String>) -> Vec<(HKEY, String, String)> {
    let mut entries = vec![];
    for (root, path) in locations(key) {
        match subkeys(root, &path) {
            Ok(keys) => entries.extend(
                keys.into_iter()
                    .map(|id| (root, format!("{path}\\{id}"), id)),
            ),
            Err(error) => errors.push(error.to_string()),
        }
    }
    entries
}

pub(super) fn installed(errors: &mut Vec<String>) -> Vec<Installed> {
    records(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        errors,
    )
    .into_iter()
    .map(|(root, path, key)| {
        let get = |name| registry_string(root, &path, name).unwrap_or_default();
        Installed {
            key,
            name: get("DisplayName"),
            publisher: get("Publisher"),
            folder: folder_of(&get("InstallLocation")),
            uninstall: get("UninstallString"),
        }
    })
    .collect()
}

/// A folder as a registry value names it, without the quotes or spaces some installers put
/// around it.
pub(super) fn folder_of(value: &str) -> PathBuf {
    PathBuf::from(value.trim().trim_matches('"').trim())
}

pub(super) fn folders(key: &str, errors: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    records(key, errors)
        .into_iter()
        .filter_map(|(root, path, id)| {
            [
                "InstallDir",
                "Install Dir",
                "InstallLocation",
                "InstallDirectory",
                "path",
            ]
            .into_iter()
            .find_map(|name| registry_string(root, &path, name))
            .map(|folder| (id, folder_of(&folder)))
        })
        .collect()
}

pub(super) fn protocol(scheme: &str) -> bool {
    locations(&format!(r"SOFTWARE\Classes\{scheme}\shell\open\command"))
        .into_iter()
        .any(|(root, path)| registry_string(root, &path, "").is_some())
}
