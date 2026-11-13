use super::*;
use windows::Win32::{
    Foundation::CloseHandle,
    Security::{
        CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, GetTokenInformation, SID_AND_ATTRIBUTES,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    Storage::FileSystem::GetShortPathNameW,
    System::Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
};

/// A folder in a new temporary folder, used as the host itself.
fn folder() -> (tempfile::TempDir, UserFolder) {
    let dir = tempfile::tempdir().unwrap();
    let folder = UserFolder::at(dir.path().to_path_buf(), None).unwrap();
    (dir, folder)
}

fn icacls(path: &Path, args: &[&str]) {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(args)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "icacls {args:?} failed");
}

fn own_token() -> OwnedHandle {
    let mut token = HANDLE::default();
    // SAFETY: token is a writable slot whose handle the OwnedHandle owns.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE | TOKEN_ASSIGN_PRIMARY,
            &mut token,
        )
        .unwrap();
        OwnedHandle::from_raw_handle(token.0)
    }
}

/// This process's token with its user's SID only able to deny: it is
/// refused what is granted to the user by name, like another user.
pub(crate) fn without_own_sid() -> OwnedHandle {
    let own = own_token();
    let own = HANDLE(own.as_raw_handle());
    let mut bytes = 0;
    // SAFETY: the buffer is aligned and as large as Windows asked for;
    // the user SID it holds is copied by CreateRestrictedToken.
    unsafe {
        let _ = GetTokenInformation(own, TokenUser, None, 0, &mut bytes);
        let mut buffer = vec![0u64; (bytes as usize).div_ceil(size_of::<u64>())];
        GetTokenInformation(
            own,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            bytes,
            &mut bytes,
        )
        .unwrap();
        let user = (*buffer.as_ptr().cast::<TOKEN_USER>()).User;
        let mut restricted = HANDLE::default();
        CreateRestrictedToken(
            own,
            DISABLE_MAX_PRIVILEGE,
            Some(&[SID_AND_ATTRIBUTES {
                Sid: user.Sid,
                Attributes: 0,
            }]),
            None,
            None,
            &mut restricted,
        )
        .unwrap();
        OwnedHandle::from_raw_handle(restricted.0)
    }
}

/// Make the folder `folder` a junction to `target` in place, as a user
/// with write access to it can (`FSCTL_SET_REPARSE_POINT`): it must be
/// empty, or already a junction. Done as this thread is.
pub(crate) fn mount(folder: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x9_00A4;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    let file = OpenOptions::new()
        .access_mode(FILE_WRITE_DATA.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(folder)?;
    let mut substitute: Vec<u16> = r"\??\".encode_utf16().collect();
    substitute.extend(target.as_os_str().encode_wide());
    let print: Vec<u16> = target.as_os_str().encode_wide().collect();
    let names = (substitute.len() + 1 + print.len() + 1) * 2;
    let mut buffer = Vec::with_capacity(16 + names);
    buffer.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend(((8 + names) as u16).to_le_bytes());
    buffer.extend(0u16.to_le_bytes());
    buffer.extend(0u16.to_le_bytes());
    buffer.extend(((substitute.len() * 2) as u16).to_le_bytes());
    buffer.extend((((substitute.len() + 1) * 2) as u16).to_le_bytes());
    buffer.extend(((print.len() * 2) as u16).to_le_bytes());
    for unit in substitute.iter().chain(&[0]).chain(&print).chain(&[0]) {
        buffer.extend(unit.to_le_bytes());
    }
    let mut bytes = 0;
    // SAFETY: the handle is open and the buffer valid for its length
    // for this synchronous call.
    unsafe {
        DeviceIoControl(
            raw(&file),
            FSCTL_SET_REPARSE_POINT,
            Some(buffer.as_ptr().cast()),
            buffer.len() as u32,
            None,
            0,
            Some(&mut bytes),
            None,
        )
    }
    .map_err(|error| io::Error::from_raw_os_error(error.code().0 & 0xFFFF))
}

/// The 8.3 short name of the file at `path`, when the volume makes one.
fn short_name(path: &Path) -> Option<String> {
    let long = HSTRING::from(path);
    let mut short = vec![0u16; 1024];
    // SAFETY: long is NUL-terminated and short a writable buffer.
    let len = unsafe { GetShortPathNameW(&long, Some(&mut short)) } as usize;
    assert!(len > 0 && len < short.len(), "{path:?}");
    let short = String::from_utf16(&short[..len]).unwrap();
    let short = short.rsplit('\\').next().unwrap().to_owned();
    (short != path.file_name().unwrap().to_str().unwrap()).then_some(short)
}

fn upload(folder: &UserFolder, name: &str, bytes: &[u8]) -> String {
    let mut part = folder.create_part(name).unwrap();
    part.write_all(bytes).unwrap();
    part.finish().unwrap()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// A file symlink, when this process may create one (elevated or
/// Developer Mode); `None` otherwise.
fn file_symlink(target: &Path, link: &Path) -> Option<()> {
    match std::os::windows::fs::symlink_file(target, link) {
        Ok(()) => Some(()),
        // ERROR_PRIVILEGE_NOT_HELD
        Err(error) if error.raw_os_error() == Some(1314) => None,
        Err(error) => panic!("symlink: {error}"),
    }
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
fn an_upload_is_written_to_part_then_renamed() {
    let (dir, folder) = folder();
    let mut part = folder.create_part("photo.jpg").unwrap();
    assert_eq!(
        part.part_path(),
        Some(dir.path().join("photo.jpg.part").as_path())
    );
    part.write_all(b"jpeg bytes").unwrap();
    assert_eq!(names(dir.path()), ["photo.jpg.part"]);
    assert_eq!(part.finish().unwrap(), "photo.jpg");
    assert_eq!(names(dir.path()), ["photo.jpg"]);
    assert_eq!(
        fs::read(dir.path().join("photo.jpg")).unwrap(),
        b"jpeg bytes"
    );
}

#[test]
fn an_upload_is_hidden_until_it_is_finished_and_then_marked_from_the_internet() {
    let (dir, folder) = folder();
    let mut part = folder.create_part("setup.exe").unwrap();
    part.write_all(b"MZ").unwrap();
    let attributes = |name: &str| {
        fs::metadata(dir.path().join(name))
            .unwrap()
            .file_attributes()
    };
    assert_ne!(attributes("setup.exe.part") & PART_MARKER.0, 0);
    assert_eq!(part.finish().unwrap(), "setup.exe");
    assert_eq!(attributes("setup.exe") & PART_MARKER.0, 0);
    assert_eq!(
        fs::read(dir.path().join("setup.exe:Zone.Identifier")).unwrap(),
        ZONE_IDENTIFIER
    );
    assert_eq!(fs::read(dir.path().join("setup.exe")).unwrap(), b"MZ");
}

#[test]
fn an_upload_past_max_path_is_finished() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f".repeat(80));
    let folder = UserFolder::at(path.clone(), None).unwrap();
    let name = format!("{}.txt", "n".repeat(196));
    assert!(path.join(file_transfer::part_name(&name)).as_os_str().len() > 260);
    assert_eq!(upload(&folder, &name, b"long"), name);
    let numbered = file_transfer::collision_name(&name, 1);
    assert_eq!(upload(&folder, &name, b"longer"), numbered);
    assert_eq!(names(&path), [numbered.clone(), name.clone()]);
    let (mut file, len) = folder.open_file(&name).unwrap();
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert_eq!((bytes.as_slice(), len), (b"long".as_slice(), 4));
    // The numbered copy is longer than any name an upload may have.
    assert_eq!(folder.list().unwrap().len(), 2);
    assert_eq!(folder.open_file(&numbered).unwrap().1, 6);
    assert!(folder.free_bytes().unwrap() > 0);
    let (free, total) = folder.volume_bytes().unwrap();
    assert!(0 < free && free <= total, "{free} of {total}");
    drop(file);
    // An incomplete one is deleted there too.
    let mut part = folder.create_part(&name).unwrap();
    part.write_all(b"partial").unwrap();
    drop(part);
    assert_eq!(names(&path), [numbered, name]);
}

#[test]
fn opening_the_folder_sweeps_only_its_own_stale_uploads() {
    let (dir, folder) = folder();
    // Left by a host that stopped mid-upload.
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .attributes(PART_MARKER.0)
        .open(dir.path().join("stale.txt.part"))
        .unwrap();
    // A browser's download in progress, not marked.
    fs::write(dir.path().join("browser.zip.part"), "zip").unwrap();
    let mut live = folder.create_part("live.txt").unwrap();
    live.write_all(b"live").unwrap();
    UserFolder::at(dir.path().to_path_buf(), None).unwrap();
    assert_eq!(names(dir.path()), ["browser.zip.part", "live.txt.part"]);
    assert_eq!(live.finish().unwrap(), "live.txt");
    assert_eq!(fs::read(dir.path().join("live.txt")).unwrap(), b"live");
}

#[test]
fn names_are_the_same_exactly_when_ntfs_finds_one_by_the_other() {
    let dir = tempfile::tempdir().unwrap();
    // The Kelvin, long s, Angstrom and Ohm signs lowercase to letters
    // NTFS keeps apart from them.
    for (stored, asked) in [
        ("K", "k"),
        ("\u{e4}", "\u{c4}"),
        ("\u{212a}", "k"),
        ("\u{17f}", "s"),
        ("\u{212b}", "\u{e5}"),
        ("\u{2126}", "\u{3c9}"),
    ] {
        let stored = format!("{stored}.txt");
        let asked = format!("{asked}.txt");
        fs::write(dir.path().join(&stored), "x").unwrap();
        let found = fs::metadata(dir.path().join(&asked)).is_ok();
        assert_eq!(
            same_name(OsStr::new(&stored), OsStr::new(&asked)),
            found,
            "{stored} {asked}"
        );
        fs::remove_file(dir.path().join(&stored)).unwrap();
    }
    assert!(!same_name(OsStr::new("\u{212a}"), OsStr::new("k")));
    assert!(same_name(OsStr::new("Photo.JPG"), OsStr::new("photo.jpg")));
}

#[test]
fn a_file_opens_only_by_its_own_name() {
    let (dir, folder) = folder();
    let mut part = folder.create_part("holiday photo.jpg").unwrap();
    part.write_all(b"jpeg").unwrap();
    assert_eq!(
        upload(&folder, "long document name.txt", b"done"),
        "long document name.txt"
    );
    // Its name in another case is the same name.
    let (_file, len) = folder.open_file("LONG DOCUMENT NAME.TXT").unwrap();
    assert_eq!(len, 4);
    assert!(matches!(
        folder.open_file("holiday photo.jpg.part"),
        Err(UserFolderError::InvalidName(NameError::PartSuffix))
    ));
    // Volumes without 8.3 names have no other name to try.
    if let Some(short) = short_name(&dir.path().join("long document name.txt")) {
        assert!(
            matches!(
                folder.open_file(&short),
                Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
            ),
            "{short}"
        );
    }
    if let Some(short) = short_name(&dir.path().join("holiday photo.jpg.part")) {
        assert!(folder.open_file(&short).is_err(), "{short}");
    }
    part.discard().unwrap();
}

#[test]
fn a_compressed_file_is_listed_and_reads_as_its_data() {
    let (dir, folder) = folder();
    let bytes: Vec<u8> = b"rubylight "
        .iter()
        .copied()
        .cycle()
        .take(64 * 1024)
        .collect();
    let path = dir.path().join("notes.txt");
    fs::write(&path, &bytes).unwrap();
    let status = std::process::Command::new("compact")
        .args(["/c", "/exe:xpress4k"])
        .arg(&path)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "compact failed");
    // Compressed by the Windows Overlay Filter into a reparse point,
    // which the filter usually hides; where it shows, the file must
    // still list and read as its data.
    let listed: Vec<_> = folder
        .list()
        .unwrap()
        .into_iter()
        .map(|file| (file.name, file.size))
        .collect();
    assert_eq!(listed, [("notes.txt".to_owned(), bytes.len() as u64)]);
    let (mut file, len) = folder.open_file("notes.txt").unwrap();
    let mut read = Vec::new();
    io::Read::read_to_end(&mut file, &mut read).unwrap();
    assert_eq!(len, bytes.len() as u64);
    assert!(read == bytes, "the compressed file read as other bytes");
}

#[test]
fn a_folder_swapped_for_a_junction_keeps_the_users_rights() {
    let base = tempfile::tempdir().unwrap();
    // Authenticated Users may use everything here, junctions included,
    // except `forbidden`: only this user's SID may, which the
    // restricted token can't.
    icacls(base.path(), &["/grant", "*S-1-5-11:(OI)(CI)F"]);
    let allowed = base.path().join("allowed");
    let forbidden = base.path().join("forbidden");
    fs::create_dir(&allowed).unwrap();
    fs::create_dir(&forbidden).unwrap();
    let sid = crate::process::own_sid().unwrap();
    icacls(
        &forbidden,
        &["/inheritance:r", "/grant:r", &format!("*{sid}:(OI)(CI)F")],
    );
    fs::write(forbidden.join("secret.txt"), "secret").unwrap();
    let link = base.path().join("transfers");
    junction(&allowed, &link);
    let folder = UserFolder::at(link.clone(), Some(Arc::new(without_own_sid()))).unwrap();
    assert_eq!(upload(&folder, "a.txt", b"a"), "a.txt");
    assert_eq!(fs::read(allowed.join("a.txt")).unwrap(), b"a");

    // Swapped during an upload: the rename is checked with the user's
    // rights at the new place, and the part is still deleted.
    let mut part = folder.create_part("b.txt").unwrap();
    part.write_all(b"b").unwrap();
    fs::remove_dir(&link).unwrap();
    junction(&forbidden, &link);
    fn denied<T>(result: Result<T, UserFolderError>) -> bool {
        matches!(result, Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied)
    }
    assert!(denied(part.finish()));
    assert!(denied(folder.create_part("c.txt")));
    assert!(denied(folder.open_file("secret.txt")));
    assert!(denied(folder.list()));
    assert_eq!(names(&allowed), ["a.txt"]);
    assert_eq!(names(&forbidden), ["secret.txt"]);
}

thread_local! {
    static RESTRICTED: OwnedHandle = without_own_sid();
}

/// Game files reached as `without_own_sid`, the way the host reaches
/// them as the signed-in user.
fn restricted(work: &mut dyn FnMut()) -> io::Result<()> {
    RESTRICTED
        .with(|token| as_token(Some(token), work))
        .map_err(|error| io::Error::other(error.to_string()))
}

#[test]
fn game_files_reached_through_a_swapped_folder_keep_the_users_rights() {
    use butterpollo_core::game_files::{self, JOURNAL_FILE, Journal, Report, Scope};
    let base = tempfile::tempdir().unwrap();
    icacls(base.path(), &["/grant", "*S-1-5-11:(OI)(CI)F"]);
    // A game in a library folder, and a folder only this user's SID may
    // use, which the restricted token can't.
    let library = base.path().join("library");
    let game = library.join("game");
    fs::create_dir_all(&game).unwrap();
    fs::write(
        game.join("OptiScaler.ini"),
        "[Framerate]\r\nFramerateLimit=59\r\n",
    )
    .unwrap();
    let forbidden = base.path().join("forbidden");
    fs::create_dir_all(forbidden.join("game")).unwrap();
    let secret = forbidden.join("game").join("OptiScaler.ini");
    let streamed = "[Framerate]\r\nFramerateLimit=120\r\n";
    fs::write(&secret, streamed).unwrap();
    let sid = crate::process::own_sid().unwrap();
    icacls(
        &forbidden,
        &["/inheritance:r", "/grant:r", &format!("*{sid}:(OI)(CI)F")],
    );
    let journal = base.path().join("config").join(JOURNAL_FILE);
    let folders = game_files::folders([game.clone()], restricted);
    assert_eq!(folders.len(), 1);
    let entries = game_files::parse(&serde_json::json!([{
        "path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": "{stream_fps}"
    }]))
    .unwrap();
    let mut report = Report::default();
    let targets = game_files::targets(
        &entries,
        Some(120),
        Some(&game),
        &folders,
        restricted,
        &mut report,
    );
    assert_eq!(targets.len(), 1, "{report:?}");
    let report = game_files::apply(&journal, "phone", &targets, restricted).unwrap();
    assert_eq!(report.written.len(), 1, "{report:?}");
    assert_eq!(
        fs::read_to_string(game.join("OptiScaler.ini")).unwrap(),
        streamed
    );
    assert!(!impersonating());

    // Swapped for a junction to the forbidden folder after the checks:
    // the restore and the next write are refused with the user's rights.
    fs::rename(&library, base.path().join("moved")).unwrap();
    junction(&forbidden, &library);
    let denied = |lines: &[String]| lines.len() == 1 && lines[0].contains("(os error 5)");
    let report = game_files::restore(&journal, Scope::All, restricted).unwrap();
    assert!(denied(&report.failed), "{report:?}");
    assert_eq!(Journal::load(&journal).unwrap().changes.len(), 1);
    let report = game_files::apply(&journal, "tv", &targets, restricted).unwrap();
    assert!(denied(&report.skipped), "{report:?}");
    assert_eq!(fs::read_to_string(&secret).unwrap(), streamed);
    assert!(!impersonating());
}

#[test]
fn acting_as_a_token_ends_when_the_work_returns_or_panics() {
    let token = own_token();
    assert!(as_token(Some(&token), impersonating).unwrap());
    assert!(!impersonating());
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        as_token(Some(&token), || -> bool { panic!("work panicked") })
    }));
    assert!(panicked.is_err());
    assert!(!impersonating());
    // Without a token the work runs as the host.
    assert!(!as_token(None, impersonating).unwrap());
}

#[test]
fn an_elevated_host_acts_as_the_user_without_administrator_rights() {
    if crate::process::is_system() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    match crate::process::standard_user_token() {
        Ok(Some(_)) => {
            let folder = UserFolder::open(Some(dir.path())).unwrap();
            assert!(folder.token.is_some());
            let elevated = folder
                .as_user(|| {
                    let mut token = HANDLE::default();
                    // SAFETY: token is a writable slot whose handle the
                    // OwnedHandle owns.
                    let token = unsafe {
                        OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token)
                            .map_err(os_error)?;
                        OwnedHandle::from_raw_handle(token.0)
                    };
                    crate::process::token_elevated(&token)
                        .map_err(|error| io::Error::other(format!("{error:#}")))
                })
                .unwrap();
            assert!(!elevated);
        }
        // Not elevated, or User Account Control is off: already the user.
        Ok(None) => assert!(UserFolder::open(Some(dir.path())).unwrap().token.is_none()),
        // Elevated without the user's desktop shell: refused, not done
        // with administrator rights.
        Err(error) if error.downcast_ref::<crate::process::NoShell>().is_some() => {
            assert!(matches!(
                UserFolder::open(Some(dir.path())),
                Err(UserFolderError::NoUserShell)
            ))
        }
        Err(_) => assert!(UserFolder::open(Some(dir.path())).is_err()),
    }
}

#[test]
fn an_elevated_host_without_a_shell_opens_no_folder() {
    let dir = tempfile::tempdir().unwrap();
    let wanted = dir.path().join("transfers");
    let refused = UserFolder::open_as(Some(&wanted), || {
        standard_user(Err(crate::process::NoShell.into()))
    });
    assert!(matches!(refused, Err(UserFolderError::NoUserShell)));
    // Not created with the host's administrator rights instead.
    assert!(!wanted.exists());
    let message = UserFolderError::NoUserShell.to_string();
    assert!(
        message.contains("desktop shell is not running"),
        "{message}"
    );
    assert!(
        message.contains("without administrator rights"),
        "{message}"
    );
}

#[test]
fn other_token_failures_stay_io_errors_with_their_cause() {
    let failed = standard_user(Err(anyhow::anyhow!(
        "the Windows desktop shell belongs to another user"
    )));
    match failed {
        Err(UserFolderError::Io(error)) => {
            assert!(
                error.to_string().contains("belongs to another user"),
                "{error}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(standard_user(Ok(None)), Ok(None)));
}

#[test]
fn a_relative_custom_folder_is_refused_before_the_user_is_looked_up() {
    let refused = UserFolder::open_as(Some(Path::new("Downloads")), || {
        panic!("the user's token was looked up for a refused folder")
    });
    assert!(matches!(
        refused,
        Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput
    ));
}

#[test]
fn paths_get_the_verbatim_prefix_once() {
    let verbatim = |path: &str| String::from_utf16(&verbatim(Path::new(path)).unwrap()).unwrap();
    assert_eq!(verbatim(r"C:\a\..\b"), r"\\?\C:\b");
    assert_eq!(verbatim(r"\\server\share\x"), r"\\?\UNC\server\share\x");
    assert_eq!(verbatim(r"\\.\C:\x"), r"\\?\C:\x");
    assert_eq!(verbatim(r"\\?\C:\x"), r"\\?\C:\x");
}

#[test]
fn a_session_without_a_user_is_no_console_user() {
    assert!(no_signed_in_user(HRESULT::from_win32(ERROR_NO_TOKEN.0)));
    assert!(no_signed_in_user(HRESULT::from_win32(
        ERROR_FILE_NOT_FOUND.0
    )));
    assert!(!no_signed_in_user(HRESULT::from_win32(5)));
}

#[test]
fn a_taken_name_gets_the_next_number() {
    let (dir, folder) = folder();
    fs::write(dir.path().join("a.txt"), "old").unwrap();
    fs::write(dir.path().join("a (1).txt"), "older").unwrap();
    assert_eq!(upload(&folder, "a.txt", b"new"), "a (2).txt");
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"old");
    assert_eq!(fs::read(dir.path().join("a (1).txt")).unwrap(), b"older");
    assert_eq!(fs::read(dir.path().join("a (2).txt")).unwrap(), b"new");
    // A folder takes a name too.
    fs::create_dir(dir.path().join("b")).unwrap();
    assert_eq!(upload(&folder, "b", b"x"), "b (1)");
}

#[test]
fn a_name_taken_during_the_upload_is_not_overwritten() {
    let (dir, folder) = folder();
    let mut part = folder.create_part("b.txt").unwrap();
    part.write_all(b"upload").unwrap();
    fs::write(dir.path().join("b.txt"), "meanwhile").unwrap();
    assert_eq!(part.finish().unwrap(), "b (1).txt");
    assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b"meanwhile");
    assert_eq!(fs::read(dir.path().join("b (1).txt")).unwrap(), b"upload");
}

#[test]
fn uploads_of_one_name_in_flight_get_their_own_part_files() {
    let (dir, folder) = folder();
    // A `.part` file this host didn't write stays untouched.
    fs::write(dir.path().join("c.txt.part"), "stale").unwrap();
    let mut first = folder.create_part("c.txt").unwrap();
    let mut second = folder.create_part("c.txt").unwrap();
    assert_eq!(
        names(dir.path()),
        ["c (1).txt.part", "c (2).txt.part", "c.txt.part"]
    );
    first.write_all(b"first").unwrap();
    second.write_all(b"second").unwrap();
    assert_eq!(second.finish().unwrap(), "c.txt");
    assert_eq!(first.finish().unwrap(), "c (1).txt");
    assert_eq!(fs::read(dir.path().join("c.txt")).unwrap(), b"second");
    assert_eq!(fs::read(dir.path().join("c (1).txt")).unwrap(), b"first");
    assert_eq!(fs::read(dir.path().join("c.txt.part")).unwrap(), b"stale");
}

#[test]
fn an_incomplete_upload_leaves_nothing_behind() {
    let (dir, folder) = folder();
    let mut part = folder.create_part("d.bin").unwrap();
    part.write_all(&[7; 4096]).unwrap();
    part.discard().unwrap();
    assert!(names(dir.path()).is_empty());
    // Dropped without finishing, as when the request fails part way.
    let mut part = folder.create_part("d.bin").unwrap();
    part.write_all(&[7; 4096]).unwrap();
    drop(part);
    assert!(names(dir.path()).is_empty());
}

#[test]
fn invalid_names_create_nothing_and_open_nothing() {
    let (dir, folder) = folder();
    fs::write(dir.path().join("secret"), "x").unwrap();
    for name in ["..", "..\\secret", "x:y", "CON.txt", "a.", ""] {
        assert!(
            matches!(
                folder.create_part(name),
                Err(UserFolderError::InvalidName(_))
            ),
            "{name:?}"
        );
        assert!(
            matches!(folder.open_file(name), Err(UserFolderError::InvalidName(_))),
            "{name:?}"
        );
    }
    assert_eq!(names(dir.path()), ["secret"]);
}

#[test]
fn only_plain_files_are_listed_and_opened() {
    let (dir, folder) = folder();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("elsewhere.txt"), "outside").unwrap();
    fs::write(dir.path().join("b.txt"), "bb").unwrap();
    fs::write(dir.path().join("a.txt"), "a").unwrap();
    fs::write(dir.path().join("e.txt.part"), "partial").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    junction(outside.path(), &dir.path().join("junction"));
    let linked = file_symlink(
        &outside.path().join("elsewhere.txt"),
        &dir.path().join("link.txt"),
    );

    let files = folder.list().unwrap();
    let listed: Vec<_> = files.iter().map(|f| (f.name.as_str(), f.size)).collect();
    assert_eq!(listed, [("a.txt", 1), ("b.txt", 2)]);
    let now_ms = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    for file in &files {
        assert!(file.modified_unix_ms > now_ms - 600_000, "{file:?}");
        assert!(file.modified_unix_ms <= now_ms + 1000, "{file:?}");
    }

    let (mut file, len) = folder.open_file("b.txt").unwrap();
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert_eq!((bytes.as_slice(), len), (b"bb".as_slice(), 2));
    for name in ["sub", "junction"] {
        assert!(
            matches!(folder.open_file(name), Err(UserFolderError::NotPlainFile)),
            "{name}"
        );
    }
    if linked.is_some() {
        assert!(matches!(
            folder.open_file("link.txt"),
            Err(UserFolderError::NotPlainFile)
        ));
    }
    assert!(matches!(
        folder.open_file("missing.txt"),
        Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
    ));
}

#[test]
fn free_space_is_reported() {
    let (_dir, folder) = folder();
    assert!(folder.free_bytes().unwrap() > 0);
}

#[test]
fn a_relative_custom_folder_is_refused() {
    assert!(matches!(
        UserFolder::open(Some(Path::new("Downloads"))),
        Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput
    ));
}

fn impersonating() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: token is a writable slot, closed below when opened.
    unsafe {
        let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token).is_ok();
        if opened {
            let _ = CloseHandle(token);
        }
        opened
    }
}

#[test]
fn impersonation_ends_after_success_failure_and_panic() {
    let dir = tempfile::tempdir().unwrap();
    let folder = UserFolder {
        path: dir.path().to_path_buf(),
        token: Some(Arc::new(own_token())),
        _pin: None,
    };
    assert!(!impersonating());
    assert!(folder.as_user(|| Ok(impersonating())).unwrap());
    assert!(!impersonating());
    let failed: Result<(), _> = folder.as_user(|| Err(io::Error::other("failed")));
    assert!(failed.is_err());
    assert!(!impersonating());
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        folder.as_user(|| -> io::Result<()> { panic!("work panicked") })
    }));
    assert!(panicked.is_err());
    assert!(!impersonating());
    // The handles opened as the user write after it ends.
    let mut part = folder.create_part("f.txt").unwrap();
    assert!(!impersonating());
    part.write_all(b"written").unwrap();
    assert_eq!(part.finish().unwrap(), "f.txt");
    assert!(!impersonating());
}

#[test]
#[ignore = "creates Downloads\\Rubylight in this user's real profile"]
fn the_default_folder_is_downloads_rubylight() {
    let folder = UserFolder::open(None).unwrap();
    assert!(
        folder.path().ends_with("Downloads\\Rubylight"),
        "{:?}",
        folder.path()
    );
    assert!(folder.path().is_dir());
}

#[test]
#[ignore = "needs SYSTEM in a session nobody is signed in to (session 0 via rust/release/elevated.ps1)"]
fn without_a_signed_in_user_there_is_no_folder() {
    assert!(crate::process::is_system());
    assert!(matches!(
        UserFolder::open(None),
        Err(UserFolderError::NoConsoleUser)
    ));
}
