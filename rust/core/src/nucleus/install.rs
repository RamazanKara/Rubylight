//! The Nucleus Co-op release the host installs on first use. It is fetched,
//! not bundled: the archive is 97 MB and carries third-party binaries of its
//! own. Only this exact file is accepted.
use super::generic;
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

/// What a file or folder of a Nucleus Co-op folder is to a user's own copy
/// of it ([`part`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Nucleus' program: copied, and the copy checked against it before
    /// each run.
    Program,
    /// Copied into a new copy, then that copy's own: Nucleus and the host
    /// change it as they run.
    Seeded,
    /// What Nucleus or the host write as they run: never copied, and no
    /// part of the copy's key. A folder of it is made empty in a new copy.
    State,
}

/// The folders Nucleus Co-op 2.4.2 writes below its own as it runs: game
/// instances and their logs, handlers (mirrored on their own), game
/// profiles, downloaded covers, screenshots and descriptions, registry
/// backups, the hub's cache, downloads, handler scans and controller
/// mappings.
const STATE_FOLDERS: [&str; 11] = [
    "content",
    "handlers",
    "game profiles",
    r"gui\covers",
    r"gui\screenshots",
    r"gui\descriptions",
    r"utils\backup",
    r"webview\cache",
    "Temp",
    "allhandlers",
    "GameControllerDB",
];

/// The files Nucleus Co-op 2.4.2 writes in its folder as it runs: its
/// settings (a user's, with their nicknames, Steam IDs and the password of
/// its helper accounts: a copy starts from Nucleus' own,
/// [`super::profile::DEFAULT_SETTINGS`]), its logs, the hub's debug
/// address, Goldberg's generated interfaces and Flawless Widescreen's
/// backup of its settings.
const STATE_FILES: [&str; 7] = [
    "Settings.ini",
    "debug-log.txt",
    "crashlog.txt",
    r"webview\debugUri.txt",
    r"utils\GoldbergEmu\tools\steam_interfaces.txt",
    r"utils\FlawlessWidescreen\x64\settings_NUCLEUS_BACKUP.xml",
    r"utils\FlawlessWidescreen\x86\settings_NUCLEUS_BACKUP.xml",
];

/// Settings Nucleus Co-op ships and then changes, which hold nothing of its
/// user's: Flawless Widescreen's (its display detection and plugins).
const SEEDED: [&str; 2] = [
    r"utils\FlawlessWidescreen\x64\settings.xml",
    r"utils\FlawlessWidescreen\x86\settings.xml",
];

/// What the file or `folder` at `relative` (below a Nucleus Co-op folder,
/// `\`-separated) is to a user's own copy. The host's own files beside
/// Nucleus (`Rubylight-*`, its compatibility settings' journal) are state.
pub fn part(relative: &str, folder: bool) -> Part {
    let named = |list: &[&str]| list.iter().any(|name| name.eq_ignore_ascii_case(relative));
    if folder {
        return if named(&STATE_FOLDERS) {
            Part::State
        } else {
            Part::Program
        };
    }
    let ours = !relative.contains('\\')
        && relative
            .get(..generic::PREFIX.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(generic::PREFIX));
    if ours || named(&STATE_FILES) {
        Part::State
    } else if named(&SEEDED) {
        Part::Seeded
    } else {
        Part::Program
    }
}

/// The name of a user's copy of the Nucleus Co-op folder whose program
/// files `listing` names (with their sizes and times): a new copy once any
/// of them changes.
pub fn copy_key(listing: &str) -> String {
    hex::encode(&Sha256::digest(listing.as_bytes())[..8])
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

/// The user `sid`'s own copy named `key` ([`copy_key`]).
pub fn user_copy(data: &Path, sid: &str, key: &str) -> Option<PathBuf> {
    Some(user_folder(data, sid)?.join(key))
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
    }

    #[test]
    fn a_users_copy_takes_nucleus_program_and_leaves_its_state() {
        for (path, folder) in [
            ("NucleusCoop.exe", false),
            (r"utils\GoldbergEmu\steam_api64.dll", false),
            (r"gui\theme\default\theme.ini", false),
            (r"webview\darkreader\darkreader.js", false),
            (r"utils\User", true),
            ("gui", true),
            ("webview", true),
            // Only the root's own files are the host's.
            (r"utils\Rubylight-tool.dll", false),
        ] {
            assert_eq!(part(path, folder), Part::Program, "{path}");
        }
        for (path, folder) in [
            ("content", true),
            ("Handlers", true),
            ("game profiles", true),
            (r"gui\covers", true),
            (r"gui\screenshots", true),
            (r"gui\descriptions", true),
            (r"utils\backup", true),
            (r"webview\cache", true),
            ("Temp", true),
            ("allhandlers", true),
            ("GameControllerDB", true),
            ("debug-log.txt", false),
            ("crashlog.txt", false),
            (r"webview\debugUri.txt", false),
            (r"utils\GoldbergEmu\tools\steam_interfaces.txt", false),
            (
                r"utils\FlawlessWidescreen\x64\settings_NUCLEUS_BACKUP.xml",
                false,
            ),
            // The host's compatibility settings' journal.
            ("Rubylight-layers-S-1-5-21-1-2-3-1001.json", false),
            ("rubylight-layers-S-1-5-21-1-2-3-1001.json.tmp", false),
        ] {
            assert_eq!(part(path, folder), Part::State, "{path}");
        }
        // A user's own settings are never copied into another's copy.
        assert_eq!(part("settings.ini", false), Part::State);
        assert_eq!(
            part(r"utils\FlawlessWidescreen\x86\settings.xml", false),
            Part::Seeded
        );
        // A file is not a folder of the same name.
        assert_eq!(part("content", false), Part::Program);
    }

    #[test]
    fn a_copys_key_changes_with_its_program() {
        let key = copy_key("10\t1\tNucleusCoop.exe\n");
        assert_eq!(key.len(), 16);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()), "{key}");
        assert_eq!(key, copy_key("10\t1\tNucleusCoop.exe\n"));
        assert_ne!(key, copy_key("10\t2\tNucleusCoop.exe\n"));
        let folder = user_folder(Path::new("data"), "S-1-5-18").unwrap();
        assert_eq!(
            user_copy(Path::new("data"), "S-1-5-18", &key),
            Some(folder.join(&key))
        );
    }

    #[test]
    fn each_user_has_a_copy_of_their_own_outside_the_hosts() {
        let data = Path::new("data");
        let sid = "S-1-5-21-1004336348-1177238915-682003330-1001";
        assert_eq!(
            user_copy(data, sid, "0123456789abcdef"),
            Some(data.join("nucleus-user").join(sid).join("0123456789abcdef"))
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
