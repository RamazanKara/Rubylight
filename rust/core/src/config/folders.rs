//! `shared_folders`: the folders devices may be granted.
use serde::Deserialize;
use std::path::PathBuf;

/// A shared folder's id, as `shared_folders` defines it and a device grant
/// names it: 1 to 64 ASCII letters, digits, `-` or `_`, so that it never
/// reads as a path, and none of `__proto__`, `constructor` and `prototype`.
pub fn valid_folder_id(id: &str) -> bool {
    // Grants are a JSON object the console keeps as a JavaScript one, where
    // those names reach the prototype rather than a grant.
    !matches!(id, "__proto__" | "constructor" | "prototype")
        && (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
/// Whether `path` can be a shared folder: absolute, on a drive of this PC,
/// such as `D:\Games`. A network share (`\\server\share`), a device or
/// verbatim path (`\\.\`, `\\?\`), a relative path, `.` or `..`, and a
/// colon past the drive (an alternate data stream) are refused.
pub fn valid_folder_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
        && !path[2..].contains(':')
        && !path[3..]
            .split(['\\', '/'])
            .any(|part| part == "." || part == "..")
}

/// One entry of `shared_folders`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedFolderConfig {
    /// What device grants name; see [`valid_folder_id`].
    pub id: String,
    /// What the console and devices call the folder.
    pub name: String,
    /// See [`valid_folder_root`].
    pub path: PathBuf,
}

pub(super) fn parse_shared_folders(text: &str) -> Result<Vec<SharedFolderConfig>, String> {
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
        if !folder.path.to_str().is_some_and(valid_folder_root) {
            return Err(format!(
                "shared folder {} is not a folder on a drive of this PC: {}",
                folder.id,
                folder.path.display()
            ));
        }
    }
    Ok(folders)
}
