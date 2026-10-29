//! The signed-in user's own copy of Nucleus Co-op, which Rubylight's own
//! one-player handler runs with the user's standard rights.
//!
//! It is never a folder an elevated Nucleus runs from, and never shares one
//! with the host's copy: `<data>\nucleus-user\<SID>\<version>`, outside
//! `C:\Users` (Nucleus refuses to run from there) and removed with the
//! host's data. SYSTEM makes `nucleus-user` (only SYSTEM and Administrators
//! may change it; Users may list it) and `<SID>` (its user may add to it and
//! change what is in it, never its permissions), each with its permissions
//! from the start, and checks them when they exist, never following a
//! link. Everything below `<SID>` is done as that user: unpacking the
//! verified release, removing an older version, the handlers, the profile
//! and settings. What the user can change there runs with their rights only.
use super::install;
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::nucleus::install as pinned;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The folder holding every user's copy: SYSTEM and Administrators own and
/// change it, Users may list it (not what is inside).
const ROOT: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200a9;;;BU)";

/// A user's folder: SYSTEM and Administrators own and change it; the user
/// may list it and add to it, and change and delete what is in it, never
/// its permissions, its owner or the folder itself.
fn user_sddl(sid: &str) -> String {
    format!(
        "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200af;;;{sid})(A;OICIIO;0x1301bf;;;{sid})"
    )
}

/// The signed-in user's token, which their copy is written and run as. An
/// error without one: the host never writes their copy as itself.
pub fn user() -> Result<OwnedHandle> {
    match crate::user_folder::user_token() {
        Ok(Some(token)) => Ok(token),
        Ok(None) => bail!(
            "Nucleus Co-op's copy for the signed-in user needs the host to run as the Rubylight Windows service"
        ),
        Err(error) => Err(anyhow::anyhow!("{error}"))
            .context("finding the signed-in user, whose own Nucleus Co-op copy runs"),
    }
}

/// The SID of the user of `token`.
pub fn sid(token: &OwnedHandle) -> Result<String> {
    crate::process::token_user_sid(token)
}

/// Runs `work` as the user of `token` (None: as the host, in tests).
pub(super) fn as_user<T>(token: Option<&OwnedHandle>, work: impl FnOnce() -> T) -> Result<T> {
    let _user =
        crate::user_folder::Impersonation::begin(token).context("acting as the signed-in user")?;
    Ok(work())
}

/// What differs in tests: a test process can't give a folder to the
/// Administrators group.
struct Steps {
    create: fn(&Path, &str) -> std::io::Result<()>,
    check: fn(&Path, &[&str]) -> Result<std::fs::File>,
}
impl Steps {
    const PINNED: Self = Self {
        create: crate::process::create_folder,
        check: secured,
    };
}

/// The user `sid`'s own copy of the pinned release, made from `archive`
/// (the verified release, read as the host) when it has none, with every
/// other version and leftover in their folder removed. `token` is theirs.
pub fn ensure(
    data: &Path,
    sid: &str,
    token: &OwnedHandle,
    archive: &mut dyn FnMut() -> Result<Vec<u8>>,
) -> Result<PathBuf> {
    ensure_with(data, sid, Some(token), archive, &Steps::PINNED)
}

fn ensure_with(
    data: &Path,
    sid: &str,
    token: Option<&OwnedHandle>,
    archive: &mut dyn FnMut() -> Result<Vec<u8>>,
    steps: &Steps,
) -> Result<PathBuf> {
    static MAKING: Mutex<()> = Mutex::new(());
    let _one = MAKING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let folder = pinned::user_folder(data, sid)
        .with_context(|| format!("{sid:?} is not a Windows account's SID"))?;
    let root = pinned::user_root(data);
    let _root = made(&root, ROOT, &[], steps)?;
    // Held open, neither can be moved or replaced while the user's part runs.
    let _folder = made(&folder, &user_sddl(sid), &[sid], steps)?;
    let copy = folder.join(pinned::VERSION);
    let exe = copy.join(pinned::EXECUTABLE);
    if !as_user(token, || exe.is_file())? {
        // As the host: the archive is in its own folder.
        let archive = archive()?;
        as_user(token, || unpack(&folder, &archive))??;
    }
    as_user(token, || remove_others(&folder))??;
    Ok(copy)
}

/// The folder `path` with `sddl`'s permissions, made now or checked
/// ([`secured`]) when it is there, and open: it can't be renamed while the
/// returned handle is.
fn made(path: &Path, sddl: &str, user: &[&str], steps: &Steps) -> Result<std::fs::File> {
    match (steps.create)(path, sddl) {
        Ok(()) => {
            tracing::info!(folder = %path.display(), "Nucleus Co-op: folder for the users' own copies made")
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).with_context(|| format!("creating {}", path.display())),
    }
    (steps.check)(path, user)
}

/// The folder at `path`, open, when it is no link, SYSTEM or Administrators
/// own it, and nobody but them and `user` may change it. Otherwise someone
/// else made it: the host never writes through it.
fn secured(path: &Path, user: &[&str]) -> Result<std::fs::File> {
    use std::os::windows::fs::MetadataExt;
    let file = crate::process::open_unfollowed(path, true)
        .with_context(|| format!("opening {}", path.display()))?;
    let attributes = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?
        .file_attributes();
    let owner = crate::process::open_file_owner(&file)
        .with_context(|| format!("reading the owner of {}", path.display()))?;
    let writers = crate::process::open_file_writers(&file)
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
    verdict(path, attributes, &owner, &writers, user)?;
    Ok(file)
}

fn verdict(
    path: &Path,
    attributes: u32,
    owner: &str,
    writers: &[String],
    user: &[&str],
) -> Result<()> {
    let refused = |why: String| {
        anyhow::anyhow!(
            "{why}, so Rubylight doesn't use it for Nucleus Co-op: delete {} and start the stream again",
            path.display()
        )
    };
    if attributes & 0x400 != 0 {
        return Err(refused(format!(
            "{} is a link to elsewhere",
            path.display()
        )));
    }
    if !install::TRUSTED.contains(&owner) {
        return Err(refused(format!(
            "{} is owned by {owner}, not by SYSTEM or Administrators",
            path.display()
        )));
    }
    if let Some(other) = writers
        .iter()
        .find(|sid| !install::TRUSTED.contains(&sid.as_str()) && !user.contains(&sid.as_str()))
    {
        return Err(refused(format!(
            "{} can be changed by {other}",
            path.display()
        )));
    }
    Ok(())
}

/// Unpack the release into `<folder>\<version>`, through a partial folder
/// so a copy is whole or not there.
fn unpack(folder: &Path, archive: &[u8]) -> Result<()> {
    let staging = folder.join(format!("{}.partial", pinned::VERSION));
    remove(&staging)?;
    let unpacked = install::unpack(archive, &staging).and_then(|files| {
        ensure!(
            staging.join(pinned::EXECUTABLE).is_file(),
            "the Nucleus Co-op {} archive holds no {}",
            pinned::VERSION,
            pinned::EXECUTABLE
        );
        Ok(files)
    });
    let files = match unpacked {
        Ok(files) => files,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error.context(format!(
                "unpacking Nucleus Co-op {} for the signed-in user",
                pinned::VERSION
            )));
        }
    };
    let copy = folder.join(pinned::VERSION);
    // A copy without the executable is a broken one.
    remove(&copy)?;
    std::fs::rename(&staging, &copy)
        .with_context(|| format!("moving Nucleus Co-op into {}", copy.display()))?;
    tracing::info!(folder = %copy.display(), files, "Nucleus Co-op: the signed-in user's own copy is unpacked");
    Ok(())
}

/// Remove what else is in the user's folder: an older version, a partial
/// unpack. One still in use stays until the next time, with a warning.
fn remove_others(folder: &Path) -> Result<()> {
    for entry in
        std::fs::read_dir(folder).with_context(|| format!("listing {}", folder.display()))?
    {
        let entry = entry.with_context(|| format!("listing {}", folder.display()))?;
        if entry.file_name() == pinned::VERSION {
            continue;
        }
        let path = entry.path();
        // Neither follows a link: a link itself goes.
        let removed = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path))
        };
        match removed {
            Ok(()) => {
                tracing::info!(path = %path.display(), "Nucleus Co-op: an older copy of the signed-in user's removed")
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "Nucleus Co-op: an older copy of the signed-in user's was not removed; it is tried again next time")
            }
        }
    }
    Ok(())
}

fn remove(folder: &Path) -> Result<()> {
    match std::fs::remove_dir_all(folder) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", folder.display()))
        }
        _ => Ok(()),
    }
}

/// Make the handlers of the user's copy at `copy` those of the host's copy
/// at `folder`: the game list in the user's AppData is the one both use,
/// and Nucleus drops from it every game whose handler it doesn't have. The
/// host's are read as the host from its own folder and written as the user
/// of `token`; a handler only the copy had goes, so the host's copy never
/// sees one the user's has. Returns how many there are.
pub fn mirror_handlers(folder: &Path, copy: &Path, token: Option<&OwnedHandle>) -> Result<usize> {
    let source = folder.join("handlers");
    let mut handlers = Vec::new();
    let entries = match std::fs::read_dir(&source) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", source.display())),
    };
    for entry in entries {
        if !handler_file(&entry) {
            continue;
        }
        let bytes = std::fs::read(entry.path())
            .with_context(|| format!("reading {}", entry.path().display()))?;
        handlers.push((entry.file_name(), bytes));
    }
    let count = handlers.len();
    as_user(token, || write_handlers(&copy.join("handlers"), &handlers))??;
    Ok(count)
}

/// A handler file (`*.js`, not a link) in a handlers folder.
fn handler_file(entry: &std::fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_file())
        && Path::new(&entry.file_name())
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("js"))
}

fn write_handlers(target: &Path, handlers: &[(std::ffi::OsString, Vec<u8>)]) -> Result<()> {
    std::fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    for entry in
        std::fs::read_dir(target).with_context(|| format!("reading {}", target.display()))?
    {
        let entry = entry.with_context(|| format!("reading {}", target.display()))?;
        let name = entry.file_name();
        let js = Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("js"));
        let kept = handlers
            .iter()
            .any(|(kept, _)| kept.eq_ignore_ascii_case(&name));
        if js && !kept && !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            std::fs::remove_file(entry.path())
                .with_context(|| format!("removing {}", entry.path().display()))?;
        }
    }
    for (name, bytes) in handlers {
        let path = target.join(name);
        if std::fs::read(&path).is_ok_and(|old| old == *bytes) {
            continue;
        }
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::unstable::write::FileOptionsExt;
    use zip::write::SimpleFileOptions;

    const SID: &str = "S-1-5-21-1-2-3-1001";

    fn create(path: &Path, _: &str) -> std::io::Result<()> {
        std::fs::create_dir(path)
    }
    fn open(path: &Path, _: &[&str]) -> Result<std::fs::File> {
        Ok(crate::process::open_unfollowed(path, true)?)
    }
    const TEST: Steps = Steps {
        create,
        check: open,
    };

    fn release(extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().with_deprecated_encryption(pinned::PASSWORD);
        writer.start_file("NucleusCoop.exe", options).unwrap();
        writer.write_all(b"MZ nucleus").unwrap();
        writer.start_file("handlers/readme.txt", options).unwrap();
        writer.write_all(b"handlers").unwrap();
        for (name, data) in extra {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn junction(target: &Path, link: &Path) {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J failed");
    }

    #[test]
    fn the_copy_is_unpacked_once_from_the_release() {
        let data = tempfile::tempdir().unwrap();
        let mut reads = 0;
        let mut archive = || {
            reads += 1;
            Ok(release(&[]))
        };
        let copy = ensure_with(data.path(), SID, None, &mut archive, &TEST).unwrap();
        assert_eq!(copy, pinned::user_copy(data.path(), SID).unwrap());
        assert_eq!(
            std::fs::read(copy.join("NucleusCoop.exe")).unwrap(),
            b"MZ nucleus"
        );
        let folder = pinned::user_folder(data.path(), SID).unwrap();
        assert!(!folder.join(format!("{}.partial", pinned::VERSION)).exists());
        // Not the host's copy, nor below it.
        assert!(!copy.starts_with(data.path().join("nucleus")));
        // There already: the archive isn't read again.
        ensure_with(data.path(), SID, None, &mut archive, &TEST).unwrap();
        assert_eq!(reads, 1);
    }

    #[test]
    fn a_new_version_replaces_the_old_and_a_broken_copy_is_made_again() {
        let data = tempfile::tempdir().unwrap();
        let folder = pinned::user_folder(data.path(), SID).unwrap();
        std::fs::create_dir_all(folder.join("2.4.1").join("handlers")).unwrap();
        std::fs::write(folder.join("2.4.1").join("NucleusCoop.exe"), b"old").unwrap();
        std::fs::create_dir_all(folder.join(format!("{}.partial", pinned::VERSION))).unwrap();
        // This version without its executable: half of a copy.
        let copy = folder.join(pinned::VERSION);
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("stale.dll"), b"x").unwrap();
        let mut archive = || Ok(release(&[]));
        ensure_with(data.path(), SID, None, &mut archive, &TEST).unwrap();
        let left: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, [pinned::VERSION]);
        assert!(!copy.join("stale.dll").exists());
        assert!(copy.join("NucleusCoop.exe").is_file());
    }

    #[test]
    fn a_bad_archive_or_sid_makes_no_copy() {
        let data = tempfile::tempdir().unwrap();
        let mut traversal = || Ok(release(&[("../../evil.dll", b"x")]));
        let error = ensure_with(data.path(), SID, None, &mut traversal, &TEST).unwrap_err();
        assert!(
            format!("{error:#}").contains("would leave the Nucleus Co-op folder"),
            "{error:#}"
        );
        let folder = pinned::user_folder(data.path(), SID).unwrap();
        assert!(std::fs::read_dir(&folder).unwrap().next().is_none());
        assert!(!data.path().join("evil.dll").exists());

        let mut offline = || Err(anyhow::anyhow!("no network"));
        let error = ensure_with(data.path(), SID, None, &mut offline, &TEST).unwrap_err();
        assert_eq!(error.to_string(), "no network");

        let mut archive = || Ok(release(&[]));
        let error =
            ensure_with(data.path(), r"S-1-5-21-1\..\..", None, &mut archive, &TEST).unwrap_err();
        assert!(
            error.to_string().contains("is not a Windows account's SID"),
            "{error}"
        );
    }

    #[test]
    fn a_link_in_place_of_a_folder_is_refused() {
        let data = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        junction(elsewhere.path(), &pinned::user_root(data.path()));
        let mut archive = || Ok(release(&[]));
        let steps = Steps {
            create,
            check: secured,
        };
        let error = ensure_with(data.path(), SID, None, &mut archive, &steps).unwrap_err();
        assert!(
            format!("{error:#}").contains("is a link to elsewhere"),
            "{error:#}"
        );
        assert!(
            std::fs::read_dir(elsewhere.path())
                .unwrap()
                .next()
                .is_none()
        );

        // The user's folder too: a junction there is never written through.
        std::fs::remove_dir(pinned::user_root(data.path())).unwrap();
        std::fs::create_dir(pinned::user_root(data.path())).unwrap();
        junction(
            elsewhere.path(),
            &pinned::user_folder(data.path(), SID).unwrap(),
        );
        let mut steps_root_ok = Steps {
            create,
            check: secured,
        };
        fn root_open_user_checked(path: &Path, user: &[&str]) -> Result<std::fs::File> {
            if user.is_empty() {
                open(path, user)
            } else {
                secured(path, user)
            }
        }
        steps_root_ok.check = root_open_user_checked;
        let error = ensure_with(data.path(), SID, None, &mut archive, &steps_root_ok).unwrap_err();
        assert!(
            format!("{error:#}").contains("is a link to elsewhere"),
            "{error:#}"
        );
        assert!(
            std::fs::read_dir(elsewhere.path())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn only_the_hosts_and_the_users_own_folders_are_used() {
        let path = Path::new(r"C:\ProgramData\Butterpollo\nucleus-user");
        let trusted = ["S-1-5-18".to_string(), "S-1-5-32-544".to_string()];
        assert!(verdict(path, 0x10, "S-1-5-32-544", &trusted, &[]).is_ok());
        let user = [trusted[0].clone(), SID.to_string()];
        assert!(verdict(path, 0x10, "S-1-5-18", &user, &[SID]).is_ok());
        // Another user may not change it, nor this one where only the host may.
        let other = verdict(path, 0x10, "S-1-5-18", &["S-1-5-21-9".into()], &[SID]).unwrap_err();
        assert!(
            other.to_string().contains("can be changed by S-1-5-21-9"),
            "{other}"
        );
        assert!(
            other.to_string().ends_with(
                "delete C:\\ProgramData\\Butterpollo\\nucleus-user and start the stream again"
            ),
            "{other}"
        );
        let root = verdict(path, 0x10, "S-1-5-18", &user, &[]).unwrap_err();
        assert!(
            root.to_string()
                .contains(&format!("can be changed by {SID}")),
            "{root}"
        );
        // The user made it, or it's a link.
        let owned = verdict(path, 0x10, SID, &[], &[SID]).unwrap_err();
        assert!(
            owned.to_string().contains(&format!("is owned by {SID}")),
            "{owned}"
        );
        let link = verdict(path, 0x410, "S-1-5-18", &[], &[]).unwrap_err();
        assert!(
            link.to_string().contains("is a link to elsewhere"),
            "{link}"
        );
        let entry = verdict(path, 0x10, "S-1-5-18", &["ACE type 5".into()], &[SID]).unwrap_err();
        assert!(entry.to_string().contains("ACE type 5"), "{entry}");
    }

    #[test]
    fn the_folders_permissions_keep_the_user_to_what_is_inside() {
        // Owned by Administrators: the user can't change the permissions.
        assert!(ROOT.starts_with("O:BAD:P("));
        assert!(user_sddl(SID).starts_with("O:BAD:P("));
        // Users list the root, nothing inside it, and change nothing.
        assert!(ROOT.ends_with("(A;;0x1200a9;;;BU)"));
        // The user's folder itself: list, add files and folders, read; what
        // is inside: modify (no permissions, owner or delete-child).
        let sddl = user_sddl(SID);
        assert!(sddl.contains(&format!("(A;;0x1200af;;;{SID})")), "{sddl}");
        assert!(
            sddl.contains(&format!("(A;OICIIO;0x1301bf;;;{SID})")),
            "{sddl}"
        );
        const WRITE_DAC: u32 = 0x4_0000;
        const WRITE_OWNER: u32 = 0x8_0000;
        const DELETE: u32 = 0x1_0000;
        const DELETE_CHILD: u32 = 0x40;
        assert_eq!(
            0x1200af & (WRITE_DAC | WRITE_OWNER | DELETE | DELETE_CHILD),
            0
        );
        assert_eq!(0x1301bf & (WRITE_DAC | WRITE_OWNER | DELETE_CHILD), 0);
        assert_eq!(0x1200a9 & 0x2_0000, 0x2_0000);
        // Each makes a valid descriptor.
        for sddl in [ROOT.to_string(), sddl] {
            let folder = tempfile::tempdir().unwrap();
            let path = folder.path().join("x");
            match crate::process::create_folder(&path, &sddl) {
                Ok(()) => {
                    let file = crate::process::open_unfollowed(&path, true).unwrap();
                    assert_eq!(
                        crate::process::open_file_owner(&file).unwrap(),
                        "S-1-5-32-544"
                    );
                }
                // Not elevated: Administrators can't be made the owner.
                Err(error) => assert_eq!(error.raw_os_error(), Some(1307), "{error}"),
            }
        }
    }

    #[test]
    fn the_copys_handlers_are_the_hosts() {
        let host = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();
        let handlers = host.path().join("handlers");
        std::fs::create_dir_all(handlers.join("Puzzle")).unwrap();
        std::fs::write(handlers.join("Puzzle.js"), b"Game.GUID = 'Puzzle';").unwrap();
        std::fs::write(handlers.join("Rubylight-game.js"), b"// ours").unwrap();
        std::fs::write(handlers.join("Puzzle").join("x.dll"), b"x").unwrap();
        std::fs::write(handlers.join("notes.txt"), b"x").unwrap();
        let theirs = copy.path().join("handlers");
        std::fs::create_dir_all(&theirs).unwrap();
        // Gone from the host's: removed. Changed: replaced.
        std::fs::write(theirs.join("Removed.JS"), b"old").unwrap();
        std::fs::write(theirs.join("Puzzle.js"), b"old").unwrap();
        std::fs::write(theirs.join("readme.txt"), b"kept").unwrap();
        assert_eq!(mirror_handlers(host.path(), copy.path(), None).unwrap(), 2);
        let mut names: Vec<_> = std::fs::read_dir(&theirs)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["Puzzle.js", "Rubylight-game.js", "readme.txt"]);
        assert_eq!(
            std::fs::read(theirs.join("Puzzle.js")).unwrap(),
            b"Game.GUID = 'Puzzle';"
        );
        // A host's copy without handlers leaves the copy none.
        std::fs::remove_dir_all(&handlers).unwrap();
        assert_eq!(mirror_handlers(host.path(), copy.path(), None).unwrap(), 0);
        assert!(!theirs.join("Puzzle.js").exists());
    }

    #[test]
    fn a_linked_handler_is_not_read() {
        let host = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(elsewhere.path().join("x.js")).unwrap();
        let handlers = host.path().join("handlers");
        std::fs::create_dir_all(&handlers).unwrap();
        junction(&elsewhere.path().join("x.js"), &handlers.join("linked.js"));
        let copy = tempfile::tempdir().unwrap();
        assert_eq!(mirror_handlers(host.path(), copy.path(), None).unwrap(), 0);
    }
}
