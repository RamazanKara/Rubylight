use super::*;

/// The program files of a test's release, as it ships them.
const FILES: [(&str, &[u8]); 4] = [
    ("NucleusCoop.exe", b"MZ 2.4.2"),
    ("NucleusCoop.exe.config", b"<configuration/>"),
    ("Nucleus.Gaming.dll", b"MZ gaming"),
    (r"utils\Hooks\EasyHook64.dll", b"MZ hook"),
];

fn sha(bytes: &[u8]) -> String {
    pinned::sha256(&mut &bytes[..], u64::MAX).unwrap()
}

/// [`FILES`], the Settings.ini the release ships (Nucleus' to change) and an
/// empty folder.
fn manifest() -> Manifest {
    let mut text = String::new();
    for (path, bytes) in FILES {
        text += &format!("{} {} {path}\n", sha(bytes), bytes.len());
    }
    text += &format!("{} 3 Settings.ini\nutils\\User\\\n", sha(b"abc"));
    Manifest::parse(&text).unwrap()
}

/// The owner what this test makes gets: the Administrators group when it
/// runs elevated, else its account.
fn owner() -> String {
    crate::process::default_owner().unwrap()
}

/// Permissions for [`owner`], SYSTEM and Administrators only, and `extra`.
fn sddl(extra: &str) -> String {
    let owner = owner();
    format!("O:{owner}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{owner}){extra}")
}

/// [`TRUSTED`] and the test's [`owner`] trusted, the real drive, and a
/// memory of drives of the test's own.
fn checks<'a>(release: &'a Manifest, top: &'a Path) -> Checks<'a> {
    let me = owner();
    Checks {
        trusts: Box::new(move |_, sid| TRUSTED.contains(&sid) || sid == me),
        top: Some(top),
        release,
        volume: Box::new(volume_of),
        volumes: Box::leak(Box::default()),
    }
}

/// `path` by its long name, as Windows resolves it: a temporary folder may
/// be named by an 8.3 short name (`RUNNER~1`).
fn long(path: &Path) -> PathBuf {
    PathBuf::from(normal(std::fs::canonicalize(path).unwrap().as_os_str()))
}

/// A temporary folder (`top`) holding `Programs\Nucleus Co-op`, the test
/// release unpacked as an administrator would: with a handler, a changed
/// Settings.ini, a game instance and only SYSTEM, Administrators and the
/// test's [`owner`] able to change any of it.
struct Tree {
    _dir: tempfile::TempDir,
    top: PathBuf,
    folder: PathBuf,
}
fn tree() -> Tree {
    let dir = tempfile::tempdir().unwrap();
    let top = long(dir.path());
    let folder = top.join("Programs").join("Nucleus Co-op");
    for (path, bytes) in FILES {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    std::fs::write(folder.join("Settings.ini"), b"[Misc]\nchanged=1\n").unwrap();
    std::fs::create_dir_all(folder.join("handlers")).unwrap();
    std::fs::write(folder.join(r"handlers\Game.js"), b"var Game = {};").unwrap();
    std::fs::create_dir_all(folder.join(r"content\Game\Instance0")).unwrap();
    crate::process::restrict(&top, &sddl("")).unwrap();
    Tree {
        _dir: dir,
        top,
        folder,
    }
}

fn check(tree: &Tree, folder: &Path) -> Result<PathBuf> {
    let release = manifest();
    chosen(folder, &checks(&release, &tree.top)).map(|checked| checked.folder)
}

fn problem_of(error: &anyhow::Error) -> Option<Problem> {
    error
        .downcast_ref::<Unusable>()
        .map(|unusable| unusable.problem)
}

fn refused(tree: &Tree, folder: &Path) -> (Option<Problem>, String) {
    let error = check(tree, folder).unwrap_err();
    (problem_of(&error), format!("{error:#}"))
}

fn junction(link: &Path, target: &Path) {
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
}

#[test]
fn the_chosen_release_is_checked_whole_and_named_as_windows_resolved_it() {
    let tree = tree();
    assert_eq!(check(&tree, &tree.folder).unwrap(), tree.folder);
    // A pasted path's separators and its trailing one don't matter.
    let pasted = format!("{}\\", tree.folder.display()).replace('\\', "/");
    assert_eq!(check(&tree, Path::new(&pasted)).unwrap(), tree.folder);
    let upper = tree.folder.to_string_lossy().to_uppercase();
    assert_eq!(check(&tree, Path::new(&upper)).unwrap(), tree.folder);
}

#[test]
fn a_game_instance_folder_others_can_change_is_refused_and_links_inside_it_are_not() {
    // Nucleus' instances link to the game's own files: not walked.
    let tree = tree();
    let instance = tree.folder.join(r"content\Game");
    junction(&instance.join("Instance1"), &tree.top);
    assert_eq!(check(&tree, &tree.folder).unwrap(), tree.folder);
    // A folder its user may change: they could plant a junction or a DLL
    // in it, which the elevated Nucleus follows or runs.
    let tree = self::tree();
    let instance = tree.folder.join(r"content\Game");
    crate::process::restrict(&instance, &sddl("(A;OICI;0x1301bf;;;AU)")).unwrap();
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!(
            "{} can be changed by S-1-5-11",
            instance.display()
        )),
        "{error}"
    );
    // A link in its place.
    let tree = self::tree();
    let instance = tree.folder.join(r"content\Game");
    std::fs::remove_dir_all(&instance).unwrap();
    junction(&instance, &tree.top);
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!("{} is a link to elsewhere", instance.display())),
        "{error}"
    );
}

#[test]
fn a_dll_config_or_handler_others_can_change_is_refused_by_name() {
    for name in [
        "Nucleus.Gaming.dll",
        "NucleusCoop.exe.config",
        r"handlers\Game.js",
        r"utils\Hooks",
    ] {
        let tree = tree();
        let path = tree.folder.join(name);
        crate::process::restrict(&path, &sddl("(A;;0x1301bf;;;AU)")).unwrap();
        let (problem, error) = refused(&tree, &tree.folder);
        assert_eq!(problem, Some(Problem::Untrusted), "{error}");
        assert!(
            error.contains(&format!("{} can be changed by S-1-5-11", path.display())),
            "{error}"
        );
        assert!(
            error.contains("Unpack the release into a new folder under C:\\Program Files"),
            "{error}"
        );
    }
}

#[test]
fn a_file_or_folder_an_untrusted_account_owns_is_refused_in_an_otherwise_good_folder() {
    // As a file unpacked into Downloads and moved keeps its user as its
    // owner: here the test's own account, trusted everywhere but there, so
    // this runs unelevated and shows each check passes the owner it read.
    let me = owner();
    for name in [
        "",
        "NucleusCoop.exe",
        "Nucleus.Gaming.dll",
        "NucleusCoop.exe.config",
        "handlers",
        r"handlers\Game.js",
        r"content\Game",
    ] {
        let tree = tree();
        let release = manifest();
        let planted = if name.is_empty() {
            tree.folder.clone()
        } else {
            tree.folder.join(name)
        };
        let mut checks = checks(&release, &tree.top);
        let (mine, at) = (me.clone(), planted.clone());
        checks.trusts = Box::new(move |path, sid| {
            (TRUSTED.contains(&sid) || sid == mine) && !(sid == mine && path == at)
        });
        let error = chosen(&tree.folder, &checks).err().unwrap();
        assert_eq!(
            problem_of(&error),
            Some(Problem::Untrusted),
            "{name}: {error:#}"
        );
        let error = format!("{error:#}");
        assert!(
            error.contains(&format!("{} is owned by {me}", planted.display())),
            "{name}: {error}"
        );
    }
}

#[test]
fn a_link_in_the_folder_or_on_its_way_is_refused_never_followed() {
    // A user's folder swapped for a junction would point the host's writes
    // elsewhere.
    let tree = tree();
    let handlers = tree.folder.join("handlers");
    std::fs::remove_dir_all(&handlers).unwrap();
    junction(&handlers, &tree.top);
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!("{} is a link to elsewhere", handlers.display())),
        "{error}"
    );
    // A junction on the way: checked through it, it could lead elsewhere by
    // the time Nucleus runs.
    let tree = self::tree();
    let link = tree.top.join("Link");
    junction(&link, &tree.top.join("Programs"));
    let through = link.join("Nucleus Co-op");
    let (problem, error) = refused(&tree, &through);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!(
            "leads to {} through a link or a short name",
            tree.folder.display()
        )),
        "{error}"
    );
    // The folder itself.
    let (problem, error) = refused(&tree, &tree.top.join("Link"));
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains("is a link to elsewhere: choose the folder itself"),
        "{error}"
    );
}

#[test]
fn a_folder_whose_parent_or_grandparent_others_can_move_is_refused() {
    // Delete children on the grandparent, delete on the parent: either
    // lets its user move the checked folder aside and put another there.
    for (above, ace) in [("", "(A;;0x40;;;AU)"), ("Programs", "(A;;0x10000;;;AU)")] {
        let tree = tree();
        let movable = tree.top.join(above);
        crate::process::restrict(&movable, &sddl(ace)).unwrap();
        let below = if above.is_empty() {
            tree.top.join("Programs")
        } else {
            tree.folder.clone()
        };
        crate::process::restrict(&below, &sddl("")).unwrap();
        let (problem, error) = refused(&tree, &tree.folder);
        assert_eq!(problem, Some(Problem::Untrusted), "{error}");
        assert!(
            error.contains(&format!(
                "{} can be moved or renamed by S-1-5-11",
                normal(movable.as_os_str())
            )),
            "{error}"
        );
    }
}

/// The capability Windows 11 gives an entry on `C:\` (RL56's PC and ours).
const C_CAPABILITY: &str = "S-1-15-3-65536-1888954469-739942743-1668119174-2468466756-4239452838-1296943325-355587736-700089176";

#[test]
fn an_app_package_or_capability_entry_above_or_in_the_folder_is_no_refusal() {
    // Such an entry grants nothing its user's own SIDs aren't allowed too,
    // here even delete children on the folder above and writing a program.
    let tree = tree();
    crate::process::restrict(
        &tree.top,
        &sddl(&format!("(A;;0x1f01ff;;;{C_CAPABILITY})(A;;0x40;;;AC)")),
    )
    .unwrap();
    crate::process::restrict(&tree.top.join("Programs"), &sddl("")).unwrap();
    crate::process::restrict(
        &tree.folder.join("Nucleus.Gaming.dll"),
        &sddl(&format!("(A;;0x1301bf;;;{C_CAPABILITY})")),
    )
    .unwrap();
    assert_eq!(check(&tree, &tree.folder).unwrap(), tree.folder);
    // A user's entry beside them still is.
    crate::process::restrict(
        &tree.top,
        &sddl(&format!("(A;;0x1f01ff;;;{C_CAPABILITY})(A;;0x40;;;AU)")),
    )
    .unwrap();
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains("can be moved or renamed by S-1-5-11"),
        "{error}"
    );
}

/// What [`left_in_checked`] accepts, by path, or the first refusal.
fn left(data: &Path, configured: &str, checks: &Checks) -> Vec<Result<PathBuf, String>> {
    left_in_checked(data, configured, checks)
        .into_iter()
        .map(|left| {
            left.map(|left| left.folder().to_path_buf())
                .map_err(|error| format!("{error:#}"))
        })
        .collect()
}

#[test]
fn settings_are_put_back_from_a_folder_nucleus_no_longer_runs_from() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = earlier(&tree);
    // Another release, with a handler others can change: no longer run from.
    std::fs::write(tree.folder.join(pinned::EXECUTABLE), b"MZ 2.3.1").unwrap();
    crate::process::restrict(&tree.folder.join("handlers"), &sddl("(A;;0x1301bf;;;AU)")).unwrap();
    assert!(check(&tree, &tree.folder).is_err());
    let configured = tree.folder.to_string_lossy().into_owned();
    assert_eq!(
        left(&data, &configured, &checks),
        [Ok(tree.folder.clone()), Ok(pinned::folder(&data))]
    );
    // Named twice, listed once; one not there is skipped.
    let earlier_folder = pinned::folder(&data).to_string_lossy().to_uppercase();
    assert_eq!(left(&data, &earlier_folder, &checks).len(), 1);
    let gone = tree.top.join("Gone");
    assert!(left(&gone, "", &checks).is_empty());
    // Held open until the restore is done: it can't be moved aside meanwhile.
    let held = left_in_checked(&data, &configured, &checks);
    let aside = tree.top.join("Programs").join("Aside");
    assert!(std::fs::rename(&tree.folder, &aside).is_err());
    drop(held);
    std::fs::rename(&tree.folder, &aside).unwrap();
    std::fs::rename(&aside, &tree.folder).unwrap();
    // A folder others can change could hold a file nobody trusted wrote.
    crate::process::restrict(&tree.folder, &sddl("(A;;0x1301bf;;;AU)")).unwrap();
    let folders = left(&data, &configured, &checks);
    let error = folders[0].as_ref().unwrap_err();
    assert!(
        error.contains("doesn't put back the settings a Nucleus Co-op session left in"),
        "{error}"
    );
    assert!(error.contains("can be changed by S-1-5-11"), "{error}");
    assert_eq!(folders[1], Ok(pinned::folder(&data)));
}

#[test]
fn settings_are_never_put_back_from_a_file_others_can_change() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = tree.top.join("none");
    let configured = tree.folder.to_string_lossy().into_owned();
    // A journal others could have written: its values would go to the hive.
    for name in [
        "Rubylight-settings.json",
        "Rubylight-layers-S-1-5-21-7.json",
        "Settings.ini",
        "Settings.ini.rubylight",
    ] {
        let file = tree.folder.join(name);
        std::fs::write(&file, b"{}").unwrap();
        assert_eq!(left(&data, &configured, &checks), [Ok(tree.folder.clone())]);
        crate::process::restrict(&file, &sddl("(A;;0x1301bf;;;AU)")).unwrap();
        let error = left(&data, &configured, &checks).remove(0).unwrap_err();
        assert!(
            error.contains(name) && error.contains("can be changed by S-1-5-11"),
            "{error}"
        );
        std::fs::remove_file(&file).unwrap();
    }
    // Nucleus' own files there are no matter: a restore never reads them.
    let other = tree.folder.join("Rubylight-notes.txt");
    std::fs::write(&other, b"").unwrap();
    crate::process::restrict(&other, &sddl("(A;;0x1301bf;;;AU)")).unwrap();
    assert_eq!(left(&data, &configured, &checks), [Ok(tree.folder.clone())]);
}

#[test]
fn a_network_folder_is_refused_before_it_is_reached() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = tree.top.join("none");
    // A network path is refused before anything reaches out to it.
    let error = left(&data, r"\\nowhere.invalid\share\Nucleus", &checks)
        .remove(0)
        .unwrap_err();
    assert!(error.contains("doesn't put back"), "{error}");
    // Not there: skipped quietly.
    let gone = tree.top.join("Programs").join("Gone");
    assert!(left(&data, &gone.to_string_lossy(), &checks).is_empty());
}

#[test]
fn settings_are_never_put_back_from_a_folder_that_can_be_swapped() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = tree.top.join("none");
    let configured = tree.folder.to_string_lossy().into_owned();
    // Its parent others can move: the folder could be moved aside and another
    // put in its place after the check.
    crate::process::restrict(&tree.top.join("Programs"), &sddl("(A;;0x40;;;AU)")).unwrap();
    crate::process::restrict(&tree.folder, &sddl("")).unwrap();
    let folders = left(&data, &configured, &checks);
    let error = folders[0].as_ref().unwrap_err();
    assert!(
        error.contains("can be moved or renamed by S-1-5-11"),
        "{error}"
    );
    crate::process::restrict(&tree.top.join("Programs"), &sddl("")).unwrap();
    assert_eq!(left(&data, &configured, &checks), [Ok(tree.folder.clone())]);
    // A link, to a folder that would pass, or on the way to it.
    let link = tree.top.join("Link");
    junction(&link, &tree.folder);
    let error = left(&data, &link.to_string_lossy(), &checks)
        .remove(0)
        .unwrap_err();
    assert!(error.contains("is a link to elsewhere"), "{error}");
    let through = tree.top.join("Through");
    junction(&through, &tree.top.join("Programs"));
    let below = through.join("Nucleus Co-op");
    let error = left(&data, &below.to_string_lossy(), &checks)
        .remove(0)
        .unwrap_err();
    assert!(error.contains("through a link or a short name"), "{error}");
    // A device path, or one through `..`.
    for path in [
        format!(r"\\?\{}", tree.folder.display()),
        format!(r"{}\..\Nucleus Co-op", tree.folder.display()),
    ] {
        assert!(std::fs::symlink_metadata(&path).is_ok(), "{path}");
        assert!(left(&data, &path, &checks)[0].is_err(), "{path}");
    }
}

#[test]
fn dots_network_paths_and_drives_other_than_fixed_ones_are_refused() {
    let tree = tree();
    let dotted = format!(
        r"{}\..\Programs\Nucleus Co-op",
        tree.top.join("Programs").display()
    );
    let here = format!(r"{}\.\Nucleus Co-op", tree.top.join("Programs").display());
    for path in [dotted.as_str(), here.as_str()] {
        let (problem, error) = refused(&tree, Path::new(path));
        assert_eq!(problem, Some(Problem::Untrusted), "{error}");
        assert!(error.contains("goes through . or .."), "{error}");
    }
    for path in [
        r"\\server\share\Nucleus Co-op",
        r"\\?\UNC\server\share\Nucleus Co-op",
        r"\\localhost\C$\Program Files\Nucleus Co-op",
        "//server/share/Nucleus Co-op",
    ] {
        let (problem, error) = refused(&tree, Path::new(path));
        assert_eq!(problem, Some(Problem::Untrusted), "{error}");
        assert!(error.contains("is a network path"), "{error}");
    }
    let device = format!(r"\\?\{}", tree.folder.display());
    let (problem, error) = refused(&tree, Path::new(&device));
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(error.contains("is a device path"), "{error}");
    // A mapped network drive, a stick, a disc: only a fixed drive.
    for (kind, says) in [
        (DRIVE_REMOTE, "a network drive"),
        (DRIVE_REMOVABLE, "a removable drive"),
        (DRIVE_CDROM, "a CD or DVD drive"),
        (DRIVE_RAMDISK, "a RAM disk"),
        (0, "a drive Windows doesn't know"),
    ] {
        let error = drive_refusal(r"Z:\Nucleus", r"Z:\", kind).unwrap();
        assert_eq!(problem_of(&error), Some(Problem::Untrusted));
        assert!(error.to_string().contains(says), "{error}");
    }
    assert!(drive_refusal(r"C:\Nucleus", r"C:\", DRIVE_FIXED).is_none());
    // A drive letter mapped for the user only is none of the service's.
    let error = drive_refusal(r"Z:\Nucleus", r"Z:\", DRIVE_NO_ROOT_DIR).unwrap();
    assert_eq!(problem_of(&error), Some(Problem::Missing));
    if let Some(free) = ('D'..='Z')
        .rev()
        .find(|letter| drive_kind(&format!("{letter}:\\")) == DRIVE_NO_ROOT_DIR)
    {
        let (problem, error) = refused(&tree, Path::new(&format!(r"{free}:\Nucleus Co-op")));
        assert_eq!(problem, Some(Problem::Missing), "{error}");
        assert!(error.contains("no drive of this PC"), "{error}");
    }
    assert_eq!(
        drive_kind(&format!("{}\\", &tree.top.to_string_lossy()[..2])),
        DRIVE_FIXED
    );
}

#[test]
fn a_short_name_is_refused_for_the_folders_own() {
    let tree = tree();
    let wide: Vec<u16> = tree
        .folder
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut short = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated and `short` writable, both outliving the call.
    let len = unsafe {
        windows::Win32::Storage::FileSystem::GetShortPathNameW(
            windows::core::PCWSTR(wide.as_ptr()),
            Some(&mut short),
        )
    } as usize;
    let short = String::from_utf16_lossy(&short[..len]);
    if len == 0 || short.eq_ignore_ascii_case(&tree.folder.to_string_lossy()) {
        eprintln!("skipped: this volume makes no 8.3 short names");
        return;
    }
    let (problem, error) = refused(&tree, Path::new(&short));
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(
        error.contains(&format!("choose {} itself", tree.folder.display())),
        "{error}"
    );
}

#[test]
fn a_swapped_program_file_is_found_though_its_size_and_time_stay() {
    let tree = tree();
    let dll = tree.folder.join("Nucleus.Gaming.dll");
    // Checked, so its answer is kept.
    check(&tree, &tree.folder).unwrap();
    let written = std::fs::metadata(&dll).unwrap().modified().unwrap();
    let same_time = |path: &Path| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(written)
            .unwrap();
    };
    // The change time ticks with the system clock (15.6 ms on some PCs): each
    // write waits for a new tick, as a real change would come later anyway.
    let tick = || std::thread::sleep(std::time::Duration::from_millis(50));
    // Written in place, its time put back: the same file id.
    tick();
    std::fs::write(&dll, b"MZ gamin!").unwrap();
    same_time(&dll);
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Version), "{error}");
    assert!(
        error.contains(&format!("{} isn't Nucleus Co-op 2.4.2's", dll.display())),
        "{error}"
    );
    tick();
    std::fs::write(&dll, b"MZ gaming").unwrap();
    same_time(&dll);
    check(&tree, &tree.folder).unwrap();
    // Another file renamed over it with the same size and time.
    tick();
    let other = tree.folder.join("other.tmp");
    std::fs::write(&other, b"MZ gamin?").unwrap();
    same_time(&other);
    std::fs::rename(&other, &dll).unwrap();
    assert_eq!(
        std::fs::metadata(&dll).unwrap().modified().unwrap(),
        written
    );
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Version), "{error}");
}

#[test]
fn a_program_file_held_open_to_write_is_refused_though_its_answer_is_kept() {
    let tree = tree();
    // Checked, so its answer is kept.
    check(&tree, &tree.folder).unwrap();
    let dll = tree.folder.join("Nucleus.Gaming.dll");
    // Its stamp stays, but whoever holds it may write it after the check.
    let writer = std::fs::OpenOptions::new().write(true).open(&dll).unwrap();
    let error = format!("{:#}", check(&tree, &tree.folder).unwrap_err());
    assert!(
        error.contains(&format!("opening {}", dll.display())),
        "{error}"
    );
    drop(writer);
    check(&tree, &tree.folder).unwrap();
}

#[test]
fn a_held_folder_and_its_program_cant_be_moved_replaced_or_written_until_dropped() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let held = hold_checked(&tree.folder, &checks).unwrap();
    assert_eq!(held.folder(), tree.folder);
    let exe = held.program();
    assert_eq!(exe, tree.folder.join(pinned::EXECUTABLE));
    let moved = tree.top.join("Moved");
    let other = tree.folder.join("other.exe");
    std::fs::write(&other, b"MZ 2.4.2").unwrap();
    assert!(std::fs::rename(&tree.folder, &moved).is_err());
    assert!(std::fs::rename(&exe, tree.folder.join("old.exe")).is_err());
    assert!(std::fs::rename(&other, &exe).is_err());
    assert!(std::fs::OpenOptions::new().write(true).open(&exe).is_err());
    assert!(std::fs::remove_file(&exe).is_err());
    drop(held);
    std::fs::rename(&other, &exe).unwrap();
    std::fs::rename(&tree.folder, &moved).unwrap();
}

#[test]
fn a_drive_swapped_under_the_same_letter_or_on_usb_is_refused() {
    let tree = tree();
    let release = manifest();
    let first = Volume {
        name: r"\\?\Volume{11111111-2222-3333-4444-555555555555}\".into(),
        serial: 0x1234_abcd,
        usb: false,
    };
    let now = std::sync::Arc::new(Mutex::new(first.clone()));
    let mut checks = checks(&release, &tree.top);
    let seen = now.clone();
    checks.volume = Box::new(move |_| Ok(seen.lock().unwrap().clone()));
    hold_checked(&tree.folder, &checks).unwrap();
    // Another disk with the same files and owners, or the same serial
    // number on a volume Windows names otherwise.
    for swapped in [
        Volume {
            serial: 0x0bad_f00d,
            ..first.clone()
        },
        Volume {
            name: r"\\?\Volume{99999999-2222-3333-4444-555555555555}\".into(),
            ..first.clone()
        },
    ] {
        *now.lock().unwrap() = swapped;
        let error = hold_checked(&tree.folder, &checks).err().unwrap();
        assert_eq!(problem_of(&error), Some(Problem::Untrusted), "{error:#}");
        assert!(
            error
                .to_string()
                .contains("isn't the one Rubylight first checked it on"),
            "{error}"
        );
        // The status the console shows says so too.
        assert!(chosen(&tree.folder, &checks).is_err());
    }
    *now.lock().unwrap() = first.clone();
    hold_checked(&tree.folder, &checks).unwrap();
    // On a USB drive, which Windows may count as fixed.
    *now.lock().unwrap() = Volume {
        usb: true,
        ..first.clone()
    };
    let (problem, error) = match chosen(&tree.folder, &checks) {
        Err(error) => (problem_of(&error), format!("{error:#}")),
        Ok(_) => panic!("a folder on a USB drive was accepted"),
    };
    assert_eq!(problem, Some(Problem::Untrusted), "{error}");
    assert!(error.contains("is on a USB drive"), "{error}");
    // This PC's own drive, read as the PC does.
    let real = volume_of(&format!("{}\\", &tree.top.to_string_lossy()[..2])).unwrap();
    assert!(real.name.starts_with(r"\\?\Volume{"), "{real:?}");
}

#[test]
fn a_program_file_marked_as_downloaded_is_refused_and_keeps_its_mark() {
    for name in ["NucleusCoop.exe", "Nucleus.Gaming.dll"] {
        let tree = tree();
        let path = tree.folder.join(name);
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        std::fs::write(&stream, b"[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();
        let (problem, error) = refused(&tree, &tree.folder);
        assert_eq!(problem, Some(Problem::Blocked), "{error}");
        assert!(
            error.contains("Unblock the zip before extracting"),
            "{error}"
        );
        assert!(error.contains(&path.display().to_string()), "{error}");
        // Unblocking is the user's: the mark stays.
        assert!(std::fs::metadata(&stream).is_ok());
    }
}

#[test]
fn a_missing_or_other_program_file_is_refused_with_the_release_it_needs() {
    let tree = tree();
    let hook = tree.folder.join(r"utils\Hooks\EasyHook64.dll");
    std::fs::remove_file(&hook).unwrap();
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Version), "{error}");
    assert!(
        error.contains(&format!(
            "{} is missing from Nucleus Co-op 2.4.2",
            hook.display()
        )),
        "{error}"
    );
    let exe = tree.folder.join(pinned::EXECUTABLE);
    std::fs::write(&exe, b"MZ 2.3.1").unwrap();
    let (problem, error) = refused(&tree, &tree.folder);
    assert_eq!(problem, Some(Problem::Version), "{error}");
    assert_eq!(
        error,
        format!(
            "The NucleusCoop.exe in {} isn't Nucleus Co-op 2.4.2's: download 2.4.2 from its release page and choose its folder",
            tree.folder.display()
        )
    );
}

#[test]
fn a_folder_without_nucleus_is_refused_never_replaced_by_the_old_install() {
    let tree = tree();
    let empty = tree.top.join("Empty");
    std::fs::create_dir(&empty).unwrap();
    let file = tree.top.join("NucleusApp.zip");
    std::fs::write(&file, b"PK").unwrap();
    for (path, says) in [
        (empty.clone(), "holds no NucleusCoop.exe"),
        (tree.top.join("Gone"), "doesn't exist"),
        (file, "holds no NucleusCoop.exe"),
        (PathBuf::from("Nucleus"), "is no full folder path"),
    ] {
        let (problem, error) = refused(&tree, &path);
        assert_eq!(problem, Some(Problem::Missing), "{error}");
        assert!(error.contains(says), "{error}");
    }
}

/// `tree`'s release copied to an earlier Rubylight's install below `data`.
fn earlier(tree: &Tree) -> PathBuf {
    let data = tree.top.join("data");
    let folder = pinned::folder(&data);
    for (path, bytes) in FILES {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    crate::process::restrict(&data, &sddl("")).unwrap();
    data
}

#[test]
fn an_earlier_install_keeps_working_and_nothing_is_installed() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    let data = tree.top.join("data");
    std::fs::create_dir(&data).unwrap();
    assert_eq!(found_checked(&data, "", &checks).unwrap(), None);
    // Nothing was made below the host's data folder.
    assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);
    let data = earlier(&tree);
    let found = found_checked(&data, "", &checks).unwrap().unwrap();
    assert_eq!(found.folder, pinned::folder(&data));
    assert_eq!(found.notice(), None);
    // The folder chosen runs over it.
    let found = found_checked(&data, &tree.folder.to_string_lossy(), &checks)
        .unwrap()
        .unwrap();
    assert_eq!(found.folder, tree.folder);
}

#[test]
fn another_release_chosen_falls_back_to_the_earlier_install_with_a_notice() {
    let tree = tree();
    let release = manifest();
    let checks = checks(&release, &tree.top);
    std::fs::write(tree.folder.join(pinned::EXECUTABLE), b"MZ 2.3.1").unwrap();
    let configured = tree.folder.to_string_lossy().into_owned();
    // No earlier install: refused, saying which release it needs.
    let data = tree.top.join("none");
    let error = found_checked(&data, &configured, &checks).unwrap_err();
    assert_eq!(problem_of(&error), Some(Problem::Version));
    let data = earlier(&tree);
    let found = found_checked(&data, &configured, &checks).unwrap().unwrap();
    assert_eq!(found.folder, pinned::folder(&data));
    assert_eq!(
        found.instead.as_ref().map(|why| why.problem),
        Some(Problem::Version)
    );
    let notice = found.notice().unwrap();
    assert!(notice.contains("isn't Nucleus Co-op 2.4.2's"), "{notice}");
    assert!(
        notice.contains(&format!(
            "Until then Rubylight runs the Nucleus Co-op 2.4.2 an earlier Rubylight installed, in {}",
            pinned::folder(&data).display()
        )),
        "{notice}"
    );
    // Only another release: a folder others can change is refused.
    std::fs::write(tree.folder.join(pinned::EXECUTABLE), b"MZ 2.4.2").unwrap();
    crate::process::restrict(&tree.folder.join("handlers"), &sddl("(A;;0x1301bf;;;AU)")).unwrap();
    let error = found_checked(&data, &configured, &checks).unwrap_err();
    assert_eq!(problem_of(&error), Some(Problem::Untrusted), "{error:#}");
}

#[test]
fn only_system_and_administrators_may_change_a_folder_nucleus_runs_from() {
    let folder = Path::new(r"C:\Program Files\Nucleus");
    let trusts = |sid: &str| TRUSTED.contains(&sid);
    let trusted = &["S-1-5-18".to_string(), "S-1-5-32-544".to_string()];
    assert!(verdict(folder, "S-1-5-32-544", trusted, trusts).is_ok());
    assert!(verdict(folder, TRUSTED[2], &[], trusts).is_ok());
    let user = verdict(folder, "S-1-5-21-1-2-3-1001", &[], trusts).unwrap_err();
    assert!(
        user.to_string().contains("is owned by S-1-5-21-1-2-3-1001"),
        "{user}"
    );
    let writer = verdict(folder, "S-1-5-18", &["S-1-5-11".into()], trusts).unwrap_err();
    assert!(
        writer.to_string().contains("can be changed by S-1-5-11"),
        "{writer}"
    );
    let entry = verdict(folder, "S-1-5-18", &["ACE type 5".into()], trusts).unwrap_err();
    assert!(entry.to_string().contains("ACE type 5"), "{entry}");
}

#[test]
fn a_path_is_compared_as_windows_names_it() {
    for (path, name) in [
        (
            r"\\?\C:\Program Files\Nucleus Co-op",
            r"C:\Program Files\Nucleus Co-op",
        ),
        (r"\\?\UNC\server\share\x", r"\\server\share\x"),
        (
            r"C:/Program Files/Nucleus Co-op/",
            r"C:\Program Files\Nucleus Co-op",
        ),
        (r"\\?\C:\", r"C:\"),
        (r"C:\", r"C:\"),
    ] {
        assert_eq!(normal(std::ffi::OsStr::new(path)), name, "{path}");
    }
}
