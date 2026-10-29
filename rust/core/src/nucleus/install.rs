//! The Nucleus Co-op release the host installs on first use. It is fetched,
//! not bundled: the archive is 97 MB and carries third-party binaries of its
//! own. Only this exact file is accepted.
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

pub const VERSION: &str = "2.4.2";
pub const URL: &str = "https://github.com/SplitScreen-Me/splitscreenme-nucleus/releases/download/v2.4.2/NucleusApp.zip";
pub const SHA256: &str = "ed91880022600ae2980702e07b0fca589fa3c6f0c2925f1500570ca753006874";
pub const SIZE: u64 = 97_396_862;
/// The release archive's entries are encrypted with this published password.
pub const PASSWORD: &[u8] = b"nucleus";
pub const EXECUTABLE: &str = "NucleusCoop.exe";

/// Whether `bytes` is the pinned release.
pub fn verify(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != SIZE {
        return Err(format!(
            "The Nucleus Co-op download has {} bytes, not the {SIZE} of version {VERSION}; it was deleted.",
            bytes.len()
        ));
    }
    let digest = hex::encode(Sha256::digest(bytes));
    if digest != SHA256 {
        return Err(format!(
            "The Nucleus Co-op download's SHA-256 is {digest}, not version {VERSION}'s {SHA256}; it was deleted."
        ));
    }
    Ok(())
}

/// Where an archive entry goes below the install folder. None for an entry
/// that would leave it (an absolute path, a drive, `..`).
pub fn entry_path(name: &str) -> Option<PathBuf> {
    let name = name.replace('\\', "/");
    let path = Path::new(&name);
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(part) => {
                let text = part.to_str()?;
                // A drive or stream name (`C:`, `file:stream`) would leave the folder on Windows.
                if text.contains(':') {
                    return None;
                }
                out.push(part);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// The folder of the host's own copy below the host's data folder.
pub fn folder(data: &Path) -> PathBuf {
    data.join("nucleus").join(VERSION)
}

/// The verified release archive the host keeps beside its copy, which the
/// users' own copies are unpacked from.
pub fn archive(data: &Path) -> PathBuf {
    data.join("nucleus")
        .join(format!("NucleusApp-{VERSION}.zip"))
}

/// The folder holding each user's own copy, which runs with that user's
/// standard rights: never one an elevated Nucleus runs from.
pub fn user_root(data: &Path) -> PathBuf {
    data.join("nucleus-user")
}

/// The folder of the user `sid`'s own copies; None for a text that is no
/// SID (`S-1-` and numbers), which could name another folder.
pub fn user_folder(data: &Path, sid: &str) -> Option<PathBuf> {
    let parts = sid.strip_prefix("S-1-")?;
    let valid = sid.len() <= 184
        && parts
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    valid.then(|| user_root(data).join(sid))
}

/// The user `sid`'s own copy of this version.
pub fn user_copy(data: &Path, sid: &str) -> Option<PathBuf> {
    Some(user_folder(data, sid)?.join(VERSION))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_pinned_release_is_accepted() {
        let short = verify(b"not nucleus").unwrap_err();
        assert!(short.contains("11 bytes"), "{short}");
        let same_size = vec![0u8; SIZE as usize];
        let wrong = verify(&same_size).unwrap_err();
        assert!(wrong.contains("SHA-256"), "{wrong}");
        assert!(wrong.contains(SHA256), "{wrong}");
    }

    #[test]
    fn archive_entries_stay_in_the_install_folder() {
        assert_eq!(
            entry_path("handlers/MasterHandler.js"),
            Some(PathBuf::from("handlers").join("MasterHandler.js"))
        );
        assert_eq!(
            entry_path("gui\\theme\\x.png"),
            Some(PathBuf::from("gui").join("theme").join("x.png"))
        );
        assert_eq!(
            entry_path("./NucleusCoop.exe"),
            Some(PathBuf::from(EXECUTABLE))
        );
        for bad in [
            "../evil.dll",
            "gui/../../evil.dll",
            "/etc/x",
            "\\Windows\\x.dll",
            "C:/Windows/x.dll",
            "C:x.dll",
            "a/b:stream",
            "",
            ".",
        ] {
            assert_eq!(entry_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn each_version_has_its_own_folder() {
        assert_eq!(
            folder(Path::new("data")),
            Path::new("data").join("nucleus").join("2.4.2")
        );
        assert_eq!(
            archive(Path::new("data")),
            Path::new("data")
                .join("nucleus")
                .join("NucleusApp-2.4.2.zip")
        );
    }

    #[test]
    fn each_user_has_a_copy_of_their_own_outside_the_hosts() {
        let data = Path::new("data");
        let sid = "S-1-5-21-1004336348-1177238915-682003330-1001";
        assert_eq!(
            user_copy(data, sid),
            Some(data.join("nucleus-user").join(sid).join("2.4.2"))
        );
        // An Entra ID account.
        assert!(user_folder(data, "S-1-12-1-1234-5678").is_some());
        assert!(!user_root(data).starts_with(data.join("nucleus")));
        for bad in [
            "",
            "S-1-",
            "S-1-5--21",
            "S-1-5-21-",
            "S-1-5-21-1\\..\\..",
            "S-1-5-21-1/x",
            "..",
            "s-1-5-21-1",
            "S-1-5-21-1 ",
        ] {
            assert_eq!(user_folder(data, bad), None, "{bad:?}");
        }
        assert_eq!(
            user_folder(data, &format!("S-1-{}", "1-".repeat(100) + "1")),
            None
        );
    }
}
