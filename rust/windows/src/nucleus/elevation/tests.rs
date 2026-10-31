use super::*;

/// A copy of a Windows program with its manifest replaced by `level`'s,
/// or removed.
fn program(folder: &Path, name: &str, level: Option<&str>) -> PathBuf {
    let system = std::env::var("SystemRoot").unwrap();
    copy(
        &Path::new(&system).join(r"System32\where.exe"),
        folder,
        name,
        level,
    )
}

/// A copy of `source` with only a manifest of `level`, or no resources.
fn copy(source: &Path, folder: &Path, name: &str, level: Option<&str>) -> PathBuf {
    use windows::Win32::System::LibraryLoader::{
        BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW,
    };
    let path = folder.join(name);
    std::fs::copy(source, &path).unwrap();
    let wide = crate::text::to_wide(&path.to_string_lossy());
    let manifest = level.map(|level| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="{level}" uiAccess="false"/>
    </requestedPrivileges></security>
  </trustInfo>
</assembly>"#
        )
    });
    // SAFETY: the path is NUL-terminated, the data outlives the update,
    // and the handle is ended once.
    unsafe {
        // Every resource goes: the copy has one manifest or none.
        let update = BeginUpdateResourceW(PCWSTR(wide.as_ptr()), true).unwrap();
        if let Some(data) = &manifest {
            UpdateResourceW(
                update,
                PCWSTR(std::ptr::without_provenance(24)),
                PCWSTR(std::ptr::without_provenance(1)),
                0x0409,
                Some(data.as_ptr().cast()),
                data.len() as u32,
            )
            .unwrap();
        }
        EndUpdateResourceW(update, false).unwrap();
    }
    path
}

#[test]
fn the_embedded_manifest_level_is_read() {
    let folder = tempfile::tempdir().unwrap();
    let admin = program(folder.path(), "game.exe", Some("requireAdministrator"));
    assert_eq!(
        manifest_need(&admin).unwrap(),
        Some(Need::Administrator(Why::Manifest))
    );
    let invoker = program(folder.path(), "plain.exe", Some("asInvoker"));
    assert_eq!(manifest_need(&invoker).unwrap(), Some(Need::Nothing));
    let highest = program(folder.path(), "most.exe", Some("highestAvailable"));
    assert_eq!(manifest_need(&highest).unwrap(), Some(Need::Highest));
    // This test program's own, which Rust's linker embeds.
    assert_eq!(
        manifest_need(&std::env::current_exe().unwrap()).unwrap(),
        Some(Need::Nothing)
    );
}

#[test]
fn without_an_embedded_manifest_the_one_beside_it_counts() {
    let folder = tempfile::tempdir().unwrap();
    let bare = program(folder.path(), "bare.exe", None);
    assert_eq!(manifest_need(&bare).unwrap(), None);
    std::fs::write(
            folder.path().join("bare.exe.manifest"),
            b"\xEF\xBB\xBF<assembly><requestedExecutionLevel level=\"requireAdministrator\"/></assembly>",
        )
        .unwrap();
    assert_eq!(
        manifest_need(&bare).unwrap(),
        Some(Need::Administrator(Why::Manifest))
    );
    // An embedded one wins over the file.
    let invoker = program(folder.path(), "plain.exe", Some("asInvoker"));
    std::fs::write(
        folder.path().join("plain.exe.manifest"),
        "<requestedExecutionLevel level=\"requireAdministrator\"/>",
    )
    .unwrap();
    assert_eq!(manifest_need(&invoker).unwrap(), Some(Need::Nothing));
}

#[test]
fn a_file_that_is_no_program_is_an_error() {
    let folder = tempfile::tempdir().unwrap();
    let text = folder.path().join("game.exe");
    std::fs::write(&text, b"not a program").unwrap();
    let error = format!("{:#}", manifest_need(&text).unwrap_err());
    assert!(error.contains("game.exe's manifest"), "{error}");
}

#[test]
fn utf16_manifests_are_read() {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "<requestedExecutionLevel level='asInvoker'/>".encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    assert_eq!(rule::manifest_need(&decode(&bytes)), Some(Need::Nothing));
}

#[test]
fn where_exe_is_64_bit() {
    let system = std::env::var("SystemRoot").unwrap();
    assert!(!thirty_two_bit(&Path::new(&system).join(r"System32\where.exe")).unwrap());
    let wow = Path::new(&system).join(r"SysWOW64\where.exe");
    if wow.is_file() {
        assert!(thirty_two_bit(&wow).unwrap());
    }
}

#[test]
fn a_32_bit_program_named_like_an_installer_needs_admin_without_a_level() {
    let system = std::env::var("SystemRoot").unwrap();
    let wow = Path::new(&system).join(r"SysWOW64\where.exe");
    if !wow.is_file() {
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let sid = crate::process::own_sid().unwrap();
    // A level turns detection off.
    let invoker = copy(&wow, folder.path(), "setup.exe", Some("asInvoker"));
    assert_eq!(game_need(&invoker, &sid, None).unwrap(), Need::Nothing);
    let setup = copy(&wow, folder.path(), "game_setup.exe", None);
    assert_eq!(manifest_need(&setup).unwrap(), None);
    assert_eq!(
        game_need(&setup, &sid, None).unwrap(),
        Need::Administrator(Why::Installer)
    );
    let game = copy(&wow, folder.path(), "game.exe", None);
    assert_eq!(game_need(&game, &sid, None).unwrap(), Need::Nothing);
    // A 64-bit one is never taken for an installer.
    let system = std::env::var("SystemRoot").unwrap();
    let native = copy(
        &Path::new(&system).join(r"System32\where.exe"),
        folder.path(),
        "update.exe",
        None,
    );
    assert_eq!(game_need(&native, &sid, None).unwrap(), Need::Nothing);
}

/// The manifest beside `exe`, asking for administrator rights, then
/// `padding` spaces.
fn admin_manifest(exe: &Path, padding: usize) -> PathBuf {
    let path = external_manifest(exe);
    let mut text =
        b"<assembly><requestedExecutionLevel level=\"requireAdministrator\"/></assembly>".to_vec();
    text.resize(text.len() + padding, b' ');
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_manifest_beside_the_game_is_read_up_to_its_limit() {
    let folder = tempfile::tempdir().unwrap();
    let bare = program(folder.path(), "bare.exe", None);
    let manifest = admin_manifest(&bare, 0);
    let fits = MANIFEST_LIMIT as usize - std::fs::metadata(&manifest).unwrap().len() as usize;
    admin_manifest(&bare, fits);
    assert_eq!(
        manifest_need(&bare).unwrap(),
        Some(Need::Administrator(Why::Manifest))
    );
    // One byte more and it is refused, never read whole.
    admin_manifest(&bare, fits + 1);
    let error = format!("{:#}", manifest_need(&bare).unwrap_err());
    assert!(
        error.contains("bare.exe.manifest is larger than 64 KiB"),
        "{error}"
    );
}

#[test]
fn a_link_in_place_of_the_manifest_is_not_followed() {
    let folder = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let bare = program(folder.path(), "bare.exe", None);
    junction(elsewhere.path(), &external_manifest(&bare));
    let error = format!("{:#}", manifest_need(&bare).unwrap_err());
    assert!(
        error.contains("bare.exe.manifest is a link or a folder, not a manifest"),
        "{error}"
    );
    // A folder of that name the same.
    let other = program(folder.path(), "other.exe", None);
    std::fs::create_dir(external_manifest(&other)).unwrap();
    let error = format!("{:#}", manifest_need(&other).unwrap_err());
    assert!(error.contains("is a link or a folder"), "{error}");
}

#[test]
fn the_games_files_are_read_as_the_user() {
    let sid = crate::process::own_sid().unwrap();
    let user = crate::user_folder::tests::without_own_sid();
    let root = tempfile::tempdir().unwrap();
    // A game whose folder only this account by name may read: the
    // token without its SID, standing in for the user, is refused it.
    let named = root.path().join("named");
    crate::process::create_folder(&named, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
    let game = program(&named, "game_setup.exe", Some("asInvoker"));
    assert_eq!(game_need(&game, &sid, None).unwrap(), Need::Nothing);
    let error = format!("{:#}", game_need(&game, &sid, Some(&user)).unwrap_err());
    assert!(error.contains("game_setup.exe's manifest"), "{error}");
    assert!(error.contains("0x80070005"), "{error}");
    // A game anyone may read, whose manifest only this account may: the
    // manifest is read as the user too.
    let open = root.path().join("open");
    crate::process::create_folder(&open, "D:P(A;OICI;FA;;;WD)").unwrap();
    let bare = program(&open, "bare.exe", None);
    assert_eq!(game_need(&bare, &sid, Some(&user)).unwrap(), Need::Nothing);
    admin_manifest_of(&bare, &sid, &user);
    assert_eq!(
        game_need(&bare, &sid, None).unwrap(),
        Need::Administrator(Why::Manifest)
    );
    let error = format!("{:#}", game_need(&bare, &sid, Some(&user)).unwrap_err());
    assert!(error.contains("reading"), "{error}");
    assert!(error.contains("bare.exe.manifest"), "{error}");
    // The host is itself again once the user's reads are done.
    assert_eq!(
        game_need(&bare, &sid, None).unwrap(),
        Need::Administrator(Why::Manifest)
    );
}

/// The manifest beside `exe`, asking for administrator rights, that only
/// this account by name (`sid`) may read: `user`, standing in for the
/// user, may not open it.
fn admin_manifest_of(exe: &Path, sid: &str, user: &OwnedHandle) -> PathBuf {
    let manifest = admin_manifest(exe, 0);
    // The whole DACL is replaced: on CI the folder's Everyone entry
    // reaches the file without being marked inherited, so icacls'
    // /inheritance:r left it in place.
    crate::process::restrict(&manifest, &format!("O:{sid}D:P(A;;FA;;;{sid})")).unwrap();
    let opened = {
        let _user = crate::user_folder::Impersonation::begin(Some(user)).unwrap();
        std::fs::File::open(&manifest).map(drop)
    };
    assert!(
        opened.is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied),
        "the stand-in for the user may open {}, so these tests can't tell whose reads it is: {}",
        manifest.display(),
        String::from_utf8_lossy(
            &std::process::Command::new("icacls")
                .arg(&manifest)
                .output()
                .unwrap()
                .stdout
        )
    );
    manifest
}

#[test]
fn each_read_of_the_games_files_acts_as_the_user_anew() {
    // A read that leaves this thread as the host, as a loader that read
    // the manifest a program embeds could: the one beside it is still
    // read as the user.
    fn then_as_the_host(exe: &Path) -> Result<Option<String>> {
        let text = embedded_manifest(exe);
        // SAFETY: ends this thread's impersonation; the guard ending it
        // again does nothing.
        unsafe { windows::Win32::Security::RevertToSelf() }.unwrap();
        text
    }
    let sid = crate::process::own_sid().unwrap();
    let user = crate::user_folder::tests::without_own_sid();
    let root = tempfile::tempdir().unwrap();
    let open = root.path().join("open");
    crate::process::create_folder(&open, "D:P(A;OICI;FA;;;WD)").unwrap();
    let bare = program(&open, "bare.exe", None);
    admin_manifest_of(&bare, &sid, &user);
    assert_eq!(
        manifest_as(&bare, None, then_as_the_host).unwrap(),
        Some(Need::Administrator(Why::Manifest))
    );
    let error = format!(
        "{:#}",
        manifest_as(&bare, Some(&user), then_as_the_host).unwrap_err()
    );
    assert!(error.contains("reading"), "{error}");
    assert!(error.contains("bare.exe.manifest"), "{error}");
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

/// A folder anyone may change, whose new files anyone may only read.
fn read_only_files(path: &Path) {
    crate::process::create_folder(path, "D:P(A;;FA;;;WD)(A;OICIIO;FR;;;WD)").unwrap();
}

#[test]
fn a_game_folder_the_user_may_change_needs_nothing() {
    let game = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(game.path().join("data").join("maps")).unwrap();
    std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
    std::fs::write(game.path().join("data").join("maps").join("1.map"), b"x").unwrap();
    assert_eq!(folder_need(game.path(), None).unwrap(), Need::Nothing);
}

#[test]
fn a_file_the_user_may_not_change_needs_admin() {
    let game = tempfile::tempdir().unwrap();
    std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
    let locked = game.path().join("data");
    read_only_files(&locked);
    std::fs::write(locked.join("1.map"), b"x").unwrap();
    assert_eq!(
        folder_need(game.path(), None).unwrap(),
        Need::Administrator(Why::Folder)
    );
}

#[test]
fn links_in_the_game_folder_are_followed_as_nucleus_does() {
    let elsewhere = tempfile::tempdir().unwrap();
    let locked = elsewhere.path().join("locked");
    read_only_files(&locked);
    std::fs::write(locked.join("1.map"), b"x").unwrap();
    let game = tempfile::tempdir().unwrap();
    std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
    // .NET's walk goes into it and stops on the file it can't change.
    junction(&locked, &game.path().join("data"));
    assert_eq!(
        walk(game.path()).unwrap().need,
        Need::Administrator(Why::Folder)
    );
    // One to nothing is passed over, as .NET does.
    std::fs::remove_dir(game.path().join("data")).unwrap();
    let gone = elsewhere.path().join("gone");
    std::fs::create_dir(&gone).unwrap();
    junction(&gone, &game.path().join("data"));
    std::fs::remove_dir(&gone).unwrap();
    assert_eq!(walk(game.path()).unwrap().need, Need::Nothing);
}

#[test]
fn a_link_back_into_the_game_folder_is_walked_once() {
    let game = tempfile::tempdir().unwrap();
    std::fs::create_dir(game.path().join("data")).unwrap();
    std::fs::write(game.path().join("data").join("1.map"), b"x").unwrap();
    junction(game.path(), &game.path().join("data").join("loop"));
    junction(game.path(), &game.path().join("again"));
    assert_eq!(walk(game.path()).unwrap().need, Need::Nothing);
}

#[test]
fn the_game_folder_is_walked_as_the_user() {
    let sid = crate::process::own_sid().unwrap();
    let game = tempfile::tempdir().unwrap();
    // Only this account by name may list it: the token without its SID,
    // standing in for the user, is refused, as Nucleus run as them is.
    let named = game.path().join("named");
    crate::process::create_folder(&named, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
    std::fs::write(named.join("1.map"), b"x").unwrap();
    let user = crate::user_folder::tests::without_own_sid();
    let walked = Mutex::new(Walked::new());
    let now = Instant::now();
    assert_eq!(
        kept(&walked, &named, None, now, &mut walk).unwrap(),
        Need::Nothing
    );
    assert_eq!(
        kept(&walked, &named, Some(&user), now, &mut walk).unwrap(),
        Need::Administrator(Why::Folder)
    );
}

/// Give everyone `rights` on `path` (icacls' letters).
fn grant(path: &Path, rights: &str) {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/grant", &format!("*S-1-1-0:{rights}")])
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "icacls failed");
}

#[test]
fn the_walk_is_kept_until_what_it_looked_at_changes() {
    let game = tempfile::tempdir().unwrap();
    let data = game.path().join("data");
    std::fs::create_dir_all(data.join("maps")).unwrap();
    std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
    let walked = Mutex::new(Walked::new());
    let walks = std::cell::Cell::new(0);
    let mut counted = |folder: &Path| {
        walks.set(walks.get() + 1);
        walk(folder)
    };
    let t0 = Instant::now();
    // A clock tick after the change before it, as a launch is: a walk in
    // the tick of a change is not kept.
    let mut look = |folder: &Path, at: Instant| {
        next_tick();
        kept(&walked, folder, None, at, &mut counted).unwrap()
    };
    for _ in 0..3 {
        assert_eq!(look(game.path(), t0), Need::Nothing);
    }
    // A launch after the first adds no walk.
    assert_eq!(walks.get(), 1);
    // A file added, removed or renamed in the folder walks it again.
    std::fs::write(game.path().join("patch.dll"), b"MZ").unwrap();
    look(game.path(), t0);
    assert_eq!(walks.get(), 2);
    std::fs::rename(game.path().join("patch.dll"), game.path().join("old.dll")).unwrap();
    look(game.path(), t0);
    assert_eq!(walks.get(), 3);
    // So does one added in a folder below it.
    std::fs::write(data.join("maps").join("2.map"), b"x").unwrap();
    look(game.path(), t0);
    assert_eq!(walks.get(), 4);
    // And a folder's permissions changed.
    grant(&data, "(RX)");
    look(game.path(), t0);
    assert_eq!(walks.get(), 5);
    look(game.path(), t0);
    assert_eq!(walks.get(), 5);
    // A file added behind a junction, in the folder it leads to.
    let elsewhere = tempfile::tempdir().unwrap();
    junction(elsewhere.path(), &data.join("linked"));
    look(game.path(), t0);
    assert_eq!(walks.get(), 6);
    std::fs::write(elsewhere.path().join("3.map"), b"x").unwrap();
    look(game.path(), t0);
    assert_eq!(walks.get(), 7);
    // A file the user may not change, then may: an administrator fixed
    // what was refused, and the next launch sees it.
    let locked = game.path().join("locked");
    read_only_files(&locked);
    std::fs::write(locked.join("1.map"), b"x").unwrap();
    assert_eq!(look(game.path(), t0), Need::Administrator(Why::Folder));
    assert_eq!(look(game.path(), t0), Need::Administrator(Why::Folder));
    assert_eq!(walks.get(), 8);
    grant(&locked.join("1.map"), "M");
    assert_eq!(look(game.path(), t0), Need::Nothing);
    assert_eq!(walks.get(), 9);
    // No answer is older than KEPT_FOR: a change no change time shows.
    look(game.path(), t0 + KEPT_FOR);
    assert_eq!(walks.get(), 10);
    look(game.path(), t0 + KEPT_FOR + Duration::from_secs(1));
    assert_eq!(walks.get(), 10);
    // Another game's folder has its own answer.
    let other = tempfile::tempdir().unwrap();
    look(other.path(), t0 + KEPT_FOR);
    look(game.path(), t0 + KEPT_FOR);
    assert_eq!(walks.get(), 11);
    // A failed start forgets the game's folder, for every user.
    forget_in(&walked, game.path());
    look(game.path(), t0 + KEPT_FOR);
    assert_eq!(walks.get(), 12);
    look(other.path(), t0 + KEPT_FOR);
    assert_eq!(walks.get(), 12);
    // Answers too old to use go as another is kept.
    let kept_for = |folder: &Path| {
        walked
            .lock()
            .unwrap()
            .keys()
            .any(|(_, kept)| kept == folder)
    };
    let later = tempfile::tempdir().unwrap();
    look(later.path(), t0 + KEPT_FOR * 2 - Duration::from_secs(1));
    assert!(kept_for(game.path()) && kept_for(other.path()));
    let last = tempfile::tempdir().unwrap();
    look(last.path(), t0 + KEPT_FOR * 2);
    assert!(!kept_for(game.path()) && !kept_for(other.path()));
    assert!(kept_for(later.path()) && kept_for(last.path()));
    // A walk that fails is not kept.
    let mut failing = |_: &Path| -> Result<Walk> { anyhow::bail!("listing failed") };
    let third = tempfile::tempdir().unwrap();
    let at = Instant::now();
    assert!(kept(&walked, third.path(), None, at, &mut failing).is_err());
    assert!(!kept_for(third.path()));
}

#[test]
fn a_walk_that_saw_a_change_made_after_it_began_is_not_kept() {
    let game = tempfile::tempdir().unwrap();
    std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
    let walked = Mutex::new(Walked::new());
    let walks = std::cell::Cell::new(0);
    // A file added as the walk begins: another change in that clock
    // tick would leave the folder's change time as the walk saw it.
    let mut changing = |folder: &Path| {
        walks.set(walks.get() + 1);
        std::fs::write(folder.join(format!("{}.dll", walks.get())), b"MZ").unwrap();
        walk(folder)
    };
    let t0 = Instant::now();
    next_tick();
    kept(&walked, game.path(), None, t0, &mut changing).unwrap();
    kept(&walked, game.path(), None, t0, &mut changing).unwrap();
    assert_eq!(walks.get(), 2);
    assert!(walked.lock().unwrap().is_empty());
    // Nothing changed since the walk began: kept.
    let mut counted = |folder: &Path| {
        walks.set(walks.get() + 1);
        walk(folder)
    };
    next_tick();
    kept(&walked, game.path(), None, t0, &mut counted).unwrap();
    kept(&walked, game.path(), None, t0, &mut counted).unwrap();
    assert_eq!(walks.get(), 3);
}

#[test]
fn a_link_the_walk_passed_keeps_the_answer_while_it_leads_where_it_did() {
    use crate::user_folder::tests::mount;
    let game = tempfile::tempdir().unwrap();
    let data = game.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(game.path().join("game.exe"), b"MZ").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let locked = elsewhere.path().join("locked");
    read_only_files(&locked);
    std::fs::write(locked.join("1.map"), b"x").unwrap();
    let walked = Mutex::new(Walked::new());
    let walks = std::cell::Cell::new(0);
    let mut counted = |folder: &Path| {
        walks.set(walks.get() + 1);
        walk(folder)
    };
    let t0 = Instant::now();
    let mut look = |at: Instant| {
        next_tick();
        kept(&walked, game.path(), None, at, &mut counted).unwrap()
    };
    // A second link into a folder already walked, then led elsewhere in
    // place: walked again.
    let again = game.path().join("again");
    junction(&data, &again);
    assert_eq!(look(t0), Need::Nothing);
    assert_eq!(look(t0), Need::Nothing);
    assert_eq!(walks.get(), 1);
    mount(&again, &locked).unwrap();
    assert_eq!(look(t0), Need::Administrator(Why::Folder));
    assert_eq!(walks.get(), 2);
    std::fs::remove_dir(&again).unwrap();
    // A link to nothing, until what it names is made.
    let later = elsewhere.path().join("later");
    std::fs::create_dir(&later).unwrap();
    junction(&later, &again);
    std::fs::remove_dir(&later).unwrap();
    assert_eq!(look(t0), Need::Nothing);
    assert_eq!(look(t0), Need::Nothing);
    assert_eq!(walks.get(), 3);
    read_only_files(&later);
    std::fs::write(later.join("1.map"), b"x").unwrap();
    assert_eq!(look(t0), Need::Administrator(Why::Folder));
    assert_eq!(walks.get(), 4);
}

#[test]
fn a_folder_reached_again_is_kept_while_the_way_there_leads_there() {
    use crate::user_folder::tests::mount;
    let game = tempfile::tempdir().unwrap();
    std::fs::create_dir(game.path().join("data")).unwrap();
    // `a` leads to `outer\data`, `outer` back into the game: to its
    // `data`, walked already.
    let elsewhere = tempfile::tempdir().unwrap();
    let outer = elsewhere.path().join("outer");
    junction(game.path(), &outer);
    junction(&outer.join("data"), &game.path().join("a"));
    let walked = Mutex::new(Walked::new());
    let walks = std::cell::Cell::new(0);
    let mut counted = |folder: &Path| {
        walks.set(walks.get() + 1);
        walk(folder)
    };
    let t0 = Instant::now();
    let mut look = |at: Instant| {
        next_tick();
        kept(&walked, game.path(), None, at, &mut counted).unwrap()
    };
    assert_eq!(look(t0), Need::Nothing);
    assert_eq!(look(t0), Need::Nothing);
    assert_eq!(walks.get(), 1);
    // `outer` led elsewhere: `a` now reaches files the user may not
    // change, though neither `a` nor the game's folders changed.
    let other = elsewhere.path().join("other");
    std::fs::create_dir(&other).unwrap();
    read_only_files(&other.join("data"));
    std::fs::write(other.join("data").join("1.map"), b"x").unwrap();
    mount(&outer, &other).unwrap();
    assert_eq!(look(t0), Need::Administrator(Why::Folder));
    assert_eq!(walks.get(), 2);
}
