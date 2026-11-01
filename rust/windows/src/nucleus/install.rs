//! Nucleus Co-op on this PC: the folder the user unpacked the pinned release
//! into and chose in the console (`nucleus_path`), else the copy an earlier
//! Rubylight installed below the host's data folder. Rubylight downloads and
//! unpacks nothing: the console links to the release's page.
//!
//! Nucleus runs elevated, and the host writes its handlers, game list and
//! settings in its folder, so the folder must be one only SYSTEM and
//! Administrators can change: a folder its user could write without
//! elevation would let any program of that user run elevated. Its
//! NucleusCoop.exe must be the pinned release's, by size and SHA-256.
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::nucleus::install::{self as pinned, Problem, Unusable};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The Nucleus folder streams use: `nucleus_path` when set, which must be a
/// folder of the pinned release that only SYSTEM and Administrators may
/// change ([`Unusable`] says why not; the host's earlier copy never stands
/// in for it), else the host's earlier copy below `data` while it is there.
pub fn installed(data: &Path, configured: &str) -> Result<Option<PathBuf>> {
    installed_checked(data, configured, &Checks::PC)
}

/// The checks that differ in tests: a test can't hold the release's
/// executable, and its process may not own files as Administrators.
struct Checks {
    release: fn(&Path) -> Result<bool>,
    trusted: fn(&Path) -> Result<()>,
}
impl Checks {
    const PC: Self = Self {
        release: release_executable,
        trusted: administrators_only,
    };
}

fn installed_checked(data: &Path, configured: &str, checks: &Checks) -> Result<Option<PathBuf>> {
    let configured = configured.trim();
    if configured.is_empty() {
        // Checked as it was unpacked: an earlier Rubylight downloaded it.
        let folder = pinned::folder(data);
        return Ok(folder.join(pinned::EXECUTABLE).is_file().then_some(folder));
    }
    chosen(Path::new(configured), checks).map(Some)
}

fn refuse(problem: Problem, message: String) -> anyhow::Error {
    anyhow::Error::new(Unusable { problem, message })
}

/// The folder the user chose, when Rubylight runs Nucleus Co-op from it: a
/// folder, no link, holding the pinned release's NucleusCoop.exe (no link
/// either), and one only SYSTEM and Administrators may change.
fn chosen(folder: &Path, checks: &Checks) -> Result<PathBuf> {
    let shown = folder.display();
    let version = pinned::VERSION;
    let exe_name = pinned::EXECUTABLE;
    if !folder.is_absolute() {
        return Err(refuse(
            Problem::Missing,
            format!("{shown} is no full folder path: choose the folder that holds {exe_name}"),
        ));
    }
    // Never followed: a link would be checked here and run from elsewhere.
    let Ok(metadata) = std::fs::symlink_metadata(folder) else {
        return Err(refuse(
            Problem::Missing,
            format!(
                "{shown} doesn't exist: choose the folder you unpacked Nucleus Co-op {version} into"
            ),
        ));
    };
    if is_link(&metadata) {
        return Err(refuse(
            Problem::Untrusted,
            format!("{shown} is a link to elsewhere: choose the folder itself"),
        ));
    }
    let exe = folder.join(exe_name);
    let found = std::fs::symlink_metadata(&exe).ok();
    if !metadata.is_dir()
        || found
            .as_ref()
            .is_none_or(|exe| !exe.is_file() && !is_link(exe))
    {
        return Err(refuse(
            Problem::Missing,
            format!(
                "{shown} holds no {exe_name}: choose the folder you unpacked Nucleus Co-op {version} into"
            ),
        ));
    }
    if found.as_ref().is_some_and(is_link) {
        return Err(refuse(
            Problem::Untrusted,
            format!("{} is a link to elsewhere", exe.display()),
        ));
    }
    let release = (checks.release)(&exe).with_context(|| format!("reading {}", exe.display()))?;
    if !release {
        return Err(refuse(
            Problem::Version,
            format!(
                "The {exe_name} in {shown} isn't Nucleus Co-op {version}'s: download {version} from its release page and choose its folder"
            ),
        ));
    }
    (checks.trusted)(folder).map_err(|error| {
        refuse(
            Problem::Untrusted,
            format!("Rubylight doesn't run Nucleus Co-op from {shown}: it runs elevated, so only administrators may change its folder ({error:#}). Unpack the release into a folder under C:\\Program Files and choose that one"),
        )
    })?;
    Ok(folder.to_path_buf())
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

/// Whether `exe` is the pinned release's NucleusCoop.exe, read without
/// following a link. The answer stands while its size and time do: the
/// console asks every few seconds.
fn release_executable(exe: &Path) -> Result<bool> {
    static LAST: Mutex<Option<(PathBuf, u64, std::time::SystemTime, bool)>> = Mutex::new(None);
    let mut file = crate::process::open_unfollowed(exe, false)?;
    let metadata = file.metadata()?;
    let (size, modified) = (metadata.len(), metadata.modified()?);
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, _, _, release)) = last
        .as_ref()
        .filter(|(path, s, m, _)| path == exe && *s == size && *m == modified)
    {
        return Ok(*release);
    }
    let release = pinned::is_executable(size, &mut file)?;
    *last = Some((exe.to_path_buf(), size, modified, release));
    Ok(release)
}

/// SYSTEM's, the Administrators group's and TrustedInstaller's (what Program
/// Files holds).
pub(super) const TRUSTED: [&str; 3] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

/// Whether an elevated program may run from `folder`: neither it, its
/// NucleusCoop.exe nor its parent is a link, each is owned by [`TRUSTED`],
/// nobody else may write the folder or the executable, and nobody else may
/// move the folder out of its parent (the parent's own new folders are no
/// matter). Otherwise a program of an ordinary user could swap what runs
/// elevated, or point the host's settings writes elsewhere.
fn administrators_only(folder: &Path) -> Result<()> {
    use crate::process::{open_file_child_movers, open_file_owner, open_file_writers};
    let exe = folder.join(pinned::EXECUTABLE);
    let parent = folder.parent().filter(|p| !p.as_os_str().is_empty());
    let checked = [(folder, true), (exe.as_path(), false)]
        .into_iter()
        .chain(parent.map(|p| (p, true)));
    for (index, (path, is_folder)) in checked.enumerate() {
        let file = crate::process::open_unfollowed(path, is_folder)
            .with_context(|| format!("opening {}", path.display()))?;
        let owner = open_file_owner(&file)
            .with_context(|| format!("reading the owner of {}", path.display()))?;
        let others = if index < 2 {
            open_file_writers(&file)
        } else {
            open_file_child_movers(&file)
        }
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
        untrusted(path, &file, &owner, &others)?;
    }
    Ok(())
}

/// [`administrators_only`]'s verdict on one path, from its owner and those
/// who may change it.
fn untrusted(path: &Path, file: &std::fs::File, owner: &str, others: &[String]) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let attributes = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?
        .file_attributes();
    ensure!(
        attributes & 0x400 == 0,
        "{} is a link to elsewhere",
        path.display()
    );
    verdict(path, owner, others)
}

fn verdict(path: &Path, owner: &str, others: &[String]) -> Result<()> {
    ensure!(
        TRUSTED.contains(&owner),
        "{} is owned by {owner}, not by SYSTEM or Administrators",
        path.display()
    );
    if let Some(other) = others.iter().find(|sid| !TRUSTED.contains(&sid.as_str())) {
        bail!("{} can be changed by {other}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(exe: &Path) -> Result<bool> {
        Ok(std::fs::read(exe)? == b"MZ 2.4.2")
    }
    fn trusted(_: &Path) -> Result<()> {
        Ok(())
    }
    fn changeable(_: &Path) -> Result<()> {
        bail!("C:\\x can be changed by S-1-5-11")
    }
    const TEST: Checks = Checks { release, trusted };

    fn problem(error: &anyhow::Error) -> Option<Problem> {
        error
            .downcast_ref::<Unusable>()
            .map(|unusable| unusable.problem)
    }

    /// A folder as the user unpacked the release into it, `exe` its
    /// NucleusCoop.exe.
    fn unpacked(exe: &[u8]) -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join(pinned::EXECUTABLE), exe).unwrap();
        std::fs::create_dir(folder.path().join("handlers")).unwrap();
        folder
    }

    fn text(path: &Path) -> String {
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn the_chosen_folder_of_the_release_is_used_and_remembered_over_the_old_install() {
        let data = tempfile::tempdir().unwrap();
        let old = pinned::folder(data.path());
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(pinned::EXECUTABLE), b"MZ").unwrap();
        let own = unpacked(b"MZ 2.4.2");
        // Spaces around a pasted path don't matter.
        let configured = format!(" {} ", text(own.path()));
        assert_eq!(
            installed_checked(data.path(), &configured, &TEST).unwrap(),
            Some(own.path().to_path_buf())
        );
    }

    #[test]
    fn an_earlier_install_keeps_working_and_nothing_is_installed() {
        let data = tempfile::tempdir().unwrap();
        assert_eq!(installed_checked(data.path(), "", &TEST).unwrap(), None);
        // Nothing was made below the host's data folder.
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
        let old = pinned::folder(data.path());
        std::fs::create_dir_all(old.join("handlers")).unwrap();
        // A folder without the executable is a broken copy: none.
        assert_eq!(installed_checked(data.path(), "", &TEST).unwrap(), None);
        std::fs::write(old.join(pinned::EXECUTABLE), b"MZ").unwrap();
        assert_eq!(installed(data.path(), "").unwrap(), Some(old));
    }

    #[test]
    fn a_folder_without_nucleus_is_refused_never_replaced_by_the_old_install() {
        let data = tempfile::tempdir().unwrap();
        let old = pinned::folder(data.path());
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(pinned::EXECUTABLE), b"MZ").unwrap();
        let empty = tempfile::tempdir().unwrap();
        let gone = empty.path().join("Nucleus");
        let file = empty.path().join("NucleusApp.zip");
        std::fs::write(&file, b"PK").unwrap();
        for (path, says) in [
            (text(empty.path()), "holds no NucleusCoop.exe"),
            (text(&gone), "doesn't exist"),
            (text(&file), "holds no NucleusCoop.exe"),
            ("Nucleus".to_string(), "is no full folder path"),
        ] {
            let error = installed_checked(data.path(), &path, &TEST).unwrap_err();
            assert_eq!(problem(&error), Some(Problem::Missing), "{error:#}");
            assert!(error.to_string().contains(says), "{error:#}");
        }
    }

    #[test]
    fn another_release_is_refused_with_the_version_it_needs() {
        let data = tempfile::tempdir().unwrap();
        let other = unpacked(b"MZ 2.3.1");
        let error = installed_checked(data.path(), &text(other.path()), &TEST).unwrap_err();
        assert_eq!(problem(&error), Some(Problem::Version));
        assert_eq!(
            error.to_string(),
            format!(
                "The NucleusCoop.exe in {} isn't Nucleus Co-op 2.4.2's: download 2.4.2 from its release page and choose its folder",
                other.path().display()
            )
        );
        // The release's executable is told by its bytes, read through the
        // file itself.
        assert!(!release_executable(&other.path().join(pinned::EXECUTABLE)).unwrap());
    }

    #[test]
    fn a_link_is_refused_never_followed() {
        let data = tempfile::tempdir().unwrap();
        let own = unpacked(b"MZ 2.4.2");
        let links = tempfile::tempdir().unwrap();
        // A junction to the release's folder: checked here, it could point
        // elsewhere by the time Nucleus runs.
        let junction = links.path().join("Nucleus");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(own.path())
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let error = installed_checked(data.path(), &text(&junction), &TEST).unwrap_err();
        assert_eq!(problem(&error), Some(Problem::Untrusted), "{error:#}");
        assert!(
            error.to_string().contains("is a link to elsewhere"),
            "{error:#}"
        );
    }

    #[test]
    fn a_folder_its_user_can_change_is_refused_with_the_reason() {
        let data = tempfile::tempdir().unwrap();
        // A temporary folder is the user's, as an extracted download is.
        let own = unpacked(b"MZ 2.4.2");
        let checks = Checks {
            release,
            trusted: changeable,
        };
        let error = installed_checked(data.path(), &text(own.path()), &checks).unwrap_err();
        assert_eq!(problem(&error), Some(Problem::Untrusted));
        let error = error.to_string();
        assert!(
            error.contains("only administrators may change its folder"),
            "{error}"
        );
        assert!(error.contains("can be changed by S-1-5-11"), "{error}");
        assert!(
            error.contains("Unpack the release into a folder under C:\\Program Files"),
            "{error}"
        );
        // And on this PC, with the release's executable taken as read.
        let checks = Checks {
            release,
            trusted: administrators_only,
        };
        let error = installed_checked(data.path(), &text(own.path()), &checks).unwrap_err();
        assert_eq!(problem(&error), Some(Problem::Untrusted), "{error:#}");
    }

    #[test]
    fn only_system_and_administrators_may_change_a_folder_nucleus_runs_from() {
        let folder = Path::new(r"C:\Program Files\Nucleus");
        let trusted = &["S-1-5-18".to_string(), "S-1-5-32-544".to_string()];
        assert!(verdict(folder, "S-1-5-32-544", trusted).is_ok());
        assert!(
            verdict(
                folder,
                "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
                &[]
            )
            .is_ok()
        );
        let user = verdict(folder, "S-1-5-21-1-2-3-1001", &[]).unwrap_err();
        assert!(
            user.to_string().contains("is owned by S-1-5-21-1-2-3-1001"),
            "{user}"
        );
        let writer = verdict(folder, "S-1-5-18", &["S-1-5-11".into()]).unwrap_err();
        assert!(
            writer.to_string().contains("can be changed by S-1-5-11"),
            "{writer}"
        );
        let entry = verdict(folder, "S-1-5-18", &["ACE type 5".into()]).unwrap_err();
        assert!(entry.to_string().contains("ACE type 5"), "{entry}");
    }
}
