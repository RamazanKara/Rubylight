//! The Nucleus Co-op release the host runs. Rubylight neither bundles nor
//! downloads it: the user downloads it from its release page in their
//! browser, unpacks it and chooses its folder in the console
//! (`nucleus_path`). Only this release's NucleusCoop.exe is accepted.
use super::generic;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub const VERSION: &str = "2.4.2";
/// The release's page, where the user downloads it (`NucleusApp.zip`).
pub const RELEASE_PAGE: &str =
    "https://github.com/SplitScreen-Me/splitscreenme-nucleus/releases/tag/v2.4.2";
pub const EXECUTABLE: &str = "NucleusCoop.exe";
/// The size and SHA-256 of the release's NucleusCoop.exe, which tell it from
/// another release's: its file version still says 2.3.0.0.
pub const EXECUTABLE_SIZE: u64 = 3_008_000;
pub const EXECUTABLE_SHA256: &str =
    "a6f65dbb80fcb0d846e8f7ea3e81ccc4bcb89411cb35126045a2d5312faf6178";

/// Whether `file`, of `size` bytes, is the release's NucleusCoop.exe.
pub fn is_executable(size: u64, file: &mut impl Read) -> std::io::Result<bool> {
    if size != EXECUTABLE_SIZE {
        return Ok(false);
    }
    let mut digest = Sha256::new();
    std::io::copy(&mut file.take(EXECUTABLE_SIZE + 1), &mut digest)?;
    Ok(hex::encode(digest.finalize()) == EXECUTABLE_SHA256)
}

/// Why Rubylight doesn't run Nucleus Co-op from a folder, for the console.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// No such folder, or no NucleusCoop.exe in it.
    Missing,
    /// Its NucleusCoop.exe is another release's.
    Version,
    /// A link, or a folder others than SYSTEM and Administrators can change.
    Untrusted,
}
impl Problem {
    /// The console's name for it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Version => "version",
            Self::Untrusted => "untrusted",
        }
    }
}

/// A Nucleus Co-op folder Rubylight doesn't run, and why in the host's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    pub problem: Problem,
    pub message: String,
}
impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Unusable {}

/// The folder an earlier Rubylight installed the release into, below the
/// host's data folder. Still used while nothing else is chosen; nothing new
/// is installed there.
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

/// The ends of the host's temporary files, which it writes whole and then
/// renames over a file of Nucleus' (`Settings.rubylight.tmp` holds a user's
/// `Settings.ini`; `userprofile.json.rubylight` a game list).
const TEMPORARY: [&str; 2] = [".rubylight.tmp", ".rubylight"];

/// Every file of the pinned release's archive, `\`-separated as it unpacks,
/// and its empty folders (ending in `\`): the only program a user's copy
/// takes.
const RELEASE: &str = include_str!("release-2.4.2.txt");

/// [`RELEASE`]'s files, and its folders with every one that holds a file,
/// in lower case: Windows' names are case-insensitive.
#[derive(Default)]
struct Release {
    files: HashSet<String>,
    folders: HashSet<String>,
}

fn release() -> &'static Release {
    static PARSED: LazyLock<Release> = LazyLock::new(|| {
        let mut release = Release::default();
        for line in RELEASE.lines() {
            let line = line.to_ascii_lowercase();
            let mut folder = match line.strip_suffix('\\') {
                Some(folder) => Some(folder),
                None => {
                    release.files.insert(line.clone());
                    line.rsplit_once('\\').map(|(folder, _)| folder)
                }
            };
            while let Some(name) = folder {
                release.folders.insert(name.to_string());
                folder = name.rsplit_once('\\').map(|(parent, _)| parent);
            }
        }
        release
    });
    &PARSED
}

/// What the file or `folder` at `relative` (below a Nucleus Co-op folder,
/// `\`-separated) is to a user's own copy. Only the pinned release's own
/// files and folders ([`RELEASE`]) are program: anything else in the host's
/// folder is state, never copied into a user's copy. The host's own files
/// beside Nucleus (`Rubylight-*`, its compatibility settings' journal) and
/// its temporary files ([`TEMPORARY`]), wherever they are, are state too.
pub fn part(relative: &str, folder: bool) -> Part {
    let named = |list: &[&str]| list.iter().any(|name| name.eq_ignore_ascii_case(relative));
    let lower = relative.to_ascii_lowercase();
    if folder {
        return if !named(&STATE_FOLDERS) && release().folders.contains(&lower) {
            Part::Program
        } else {
            Part::State
        };
    }
    let ours = !relative.contains('\\')
        && relative
            .get(..generic::PREFIX.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(generic::PREFIX));
    let temporary = TEMPORARY.iter().any(|end| lower.ends_with(end));
    if ours || temporary || named(&STATE_FILES) {
        Part::State
    } else if named(&SEEDED) {
        Part::Seeded
    } else if release().files.contains(&lower) {
        Part::Program
    } else {
        Part::State
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
    fn only_the_pinned_releases_executable_is_accepted() {
        // Another file of the same size, and a size of another release.
        let same_size = vec![0u8; EXECUTABLE_SIZE as usize];
        assert!(!is_executable(EXECUTABLE_SIZE, &mut same_size.as_slice()).unwrap());
        assert!(!is_executable(11, &mut b"not nucleus".as_slice()).unwrap());
        assert_eq!(EXECUTABLE_SHA256.len(), 64);
        assert!(RELEASE_PAGE.ends_with(&format!("/v{VERSION}")));
        assert_eq!(Problem::Version.as_str(), "version");
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
            // In any case, as Windows names it.
            ("nucleuscoop.EXE", false),
            (r"utils\GoldbergEmu\steam_api64.dll", false),
            (r"gui\theme\default\theme.ini", false),
            (r"webview\darkreader\background.js", false),
            // An empty folder of the release.
            (r"utils\User", true),
            ("gui", true),
            ("webview", true),
            (r"utils\GoldbergEmu", true),
        ] {
            assert_eq!(part(path, folder), Part::Program, "{path}");
        }
        // Only the release's own: whatever else is in the host's folder is
        // never copied into a user's copy.
        for (path, folder) in [
            ("planted.dll", false),
            ("version.dll", false),
            (r"utils\Rubylight-tool.dll", false),
            (r"utils\User\custom.exe", false),
            (r"utils\GoldbergEmu\steam_api64.dll.bak", false),
            ("planted", true),
            (r"utils\planted", true),
        ] {
            assert_eq!(part(path, folder), Part::State, "{path}");
        }
        let release = release();
        assert_eq!(release.files.len(), 783);
        assert!(release.files.contains("nucleuscoop.exe"));
        assert!(release.folders.contains(r"utils\flawlesswidescreen\x64"));
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
            // The host's Settings.ini or game list, left half-way by a crash.
            ("Settings.rubylight.tmp", false),
            ("SETTINGS.RUBYLIGHT.TMP", false),
            (r"content\userprofile.json.rubylight", false),
            (r"utils\settings.rubylight.tmp", false),
        ] {
            assert_eq!(part(path, folder), Part::State, "{path}");
        }
        // A user's own settings are never copied into another's copy.
        assert_eq!(part("settings.ini", false), Part::State);
        assert_eq!(
            part(r"utils\FlawlessWidescreen\x86\settings.xml", false),
            Part::Seeded
        );
        // A file is not a folder of the same name, nor a folder a file.
        assert_eq!(part("gui", false), Part::State);
        assert_eq!(part("NucleusCoop.exe", true), Part::State);
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
