use super::*;
use std::io::{Read, Write};
use windows::{Win32::Storage::FileSystem::GetShortPathNameW, core::HSTRING};

/// A shared folder at `root` in a new temporary folder, beside `outside`.
struct Rig {
    _temp: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
    folder: SharedFolder,
}
fn rig() -> Rig {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("shared");
    let outside = temp.path().join("private");
    fs::create_dir_all(root.join("saves")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"secret").unwrap();
    fs::write(root.join("saves").join("slot1.sav"), b"slot").unwrap();
    fs::write(root.join("readme.txt"), b"hello").unwrap();
    let folder = SharedFolder::at(&root, None).unwrap();
    Rig {
        _temp: temp,
        root,
        outside,
        folder,
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

/// A symbolic link, when this process may create one (elevated or
/// Developer Mode); `None` otherwise.
fn symlink(target: &Path, link: &Path, dir: bool) -> Option<()> {
    let made = if dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    match made {
        Ok(()) => Some(()),
        // ERROR_PRIVILEGE_NOT_HELD
        Err(error) if error.raw_os_error() == Some(1314) => None,
        Err(error) => panic!("symlink: {error}"),
    }
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

fn read(folder: &SharedFolder, names: &[&str]) -> Result<Vec<u8>, UserFolderError> {
    let (mut file, len) = folder.open_file(names)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() as u64, len);
    Ok(bytes)
}

fn outside(result: Result<impl std::fmt::Debug, UserFolderError>) {
    assert!(
        matches!(result, Err(UserFolderError::OutsideFolder)),
        "{result:?}"
    );
}

#[test]
fn folders_come_first_and_links_and_uploads_in_progress_are_not_listed() {
    let rig = rig();
    fs::write(rig.root.join("b.txt"), b"bb").unwrap();
    fs::write(rig.root.join("movie.mp4.part"), b"half").unwrap();
    fs::create_dir(rig.root.join("Alpha")).unwrap();
    junction(&rig.outside, &rig.root.join("to private"));
    symlink(
        &rig.outside.join("secret.txt"),
        &rig.root.join("secret.txt"),
        false,
    );
    let entries = rig.folder.entries(&[]).unwrap();
    let listed: Vec<(&str, bool, u64)> = entries
        .iter()
        .map(|e| (e.name.as_str(), e.dir, e.size))
        .collect();
    assert_eq!(
        listed,
        [
            ("Alpha", true, 0),
            ("saves", true, 0),
            ("b.txt", false, 2),
            ("readme.txt", false, 5)
        ]
    );
    assert!(entries.iter().all(|e| e.modified_unix_ms > 0));
    let saves = rig.folder.entries(&["saves"]).unwrap();
    assert_eq!(saves[0].name, "slot1.sav");
    // A name in another case is the same folder.
    assert_eq!(rig.folder.entries(&["SAVES"]).unwrap(), saves);
}

#[test]
fn files_are_read_below_the_root_only() {
    let rig = rig();
    assert_eq!(read(&rig.folder, &["saves", "slot1.sav"]).unwrap(), b"slot");
    assert_eq!(read(&rig.folder, &["README.TXT"]).unwrap(), b"hello");
    assert!(matches!(
        read(&rig.folder, &["saves"]),
        Err(UserFolderError::NotPlainFile)
    ));
    assert!(matches!(
        read(&rig.folder, &[]),
        Err(UserFolderError::NotPlainFile)
    ));
    assert!(matches!(
        read(&rig.folder, &["missing.txt"]),
        Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
    ));
    // Names that would leave the root are refused before Windows sees them.
    for names in [
        &["..", "private", "secret.txt"][..],
        &["saves", "..", "..", "private", "secret.txt"],
        &["readme.txt:hidden"],
        &["readme.txt::$DATA"],
        &["CON"],
        &["nul.txt"],
        &["saves\\..\\..\\private\\secret.txt"],
        &["."],
    ] {
        assert!(
            matches!(
                rig.folder.open_file(names),
                Err(UserFolderError::InvalidName(_))
            ),
            "{names:?}"
        );
        assert!(
            matches!(
                rig.folder.entries(names),
                Err(UserFolderError::InvalidName(_))
            ),
            "{names:?}"
        );
    }
}

#[test]
fn a_junction_or_symlink_out_of_the_root_is_refused_every_way() {
    let rig = rig();
    junction(&rig.outside, &rig.root.join("j"));
    outside(rig.folder.entries(&["j"]));
    outside(rig.folder.open_file(&["j", "secret.txt"]));
    outside(rig.folder.upload_folder(&["j"]));
    // One back into the root is a link too.
    junction(&rig.root.join("saves"), &rig.root.join("again"));
    outside(rig.folder.entries(&["again"]));
    outside(rig.folder.open_file(&["again", "slot1.sav"]));
    if symlink(&rig.outside, &rig.root.join("d"), true).is_some() {
        outside(rig.folder.entries(&["d"]));
        outside(rig.folder.open_file(&["d", "secret.txt"]));
    }
    if symlink(
        &rig.outside.join("secret.txt"),
        &rig.root.join("s.txt"),
        false,
    )
    .is_some()
    {
        assert!(matches!(
            rig.folder.open_file(&["s.txt"]),
            Err(UserFolderError::NotPlainFile)
        ));
    }
    assert_eq!(fs::read(rig.outside.join("secret.txt")).unwrap(), b"secret");
}

#[test]
fn a_junction_to_a_name_that_only_lowercases_alike_is_refused() {
    let rig = rig();
    // A folder named with the Kelvin sign, which NTFS keeps apart from
    // "k", and a junction "kelvin" to it.
    let kelvin = rig.root.join("\u{212a}elvin");
    fs::create_dir(&kelvin).unwrap();
    fs::write(kelvin.join("secret.txt"), b"k").unwrap();
    junction(&kelvin, &rig.root.join("kelvin"));
    outside(rig.folder.open_file(&["kelvin", "secret.txt"]));
    outside(rig.folder.entries(&["kelvin"]));
    assert_eq!(
        read(&rig.folder, &["\u{212a}elvin", "secret.txt"]).unwrap(),
        b"k"
    );
}

#[test]
fn a_short_name_is_not_another_way_in() {
    let rig = rig();
    let long = rig.root.join("a long folder name");
    fs::create_dir(&long).unwrap();
    fs::write(long.join("a long file name.txt"), b"x").unwrap();
    // Volumes without 8.3 names have no other name to try.
    if let Some(short) = short_name(&long) {
        outside(rig.folder.entries(&[&short]));
        outside(rig.folder.open_file(&[&short, "a long file name.txt"]));
    }
    if let Some(short) = short_name(&long.join("a long file name.txt")) {
        outside(rig.folder.open_file(&["a long folder name", &short]));
    }
    assert_eq!(
        read(&rig.folder, &["a long folder name", "a long file name.txt"]).unwrap(),
        b"x"
    );
}

#[test]
fn a_root_that_is_a_junction_is_the_folder_it_leads_to() {
    let rig = rig();
    let link = rig._temp.path().join("link to shared");
    junction(&rig.root, &link);
    let folder = SharedFolder::at(&link, None).unwrap();
    assert_eq!(
        folder.root().to_str().unwrap().to_lowercase(),
        rig.folder.root().to_str().unwrap().to_lowercase()
    );
    assert_eq!(read(&folder, &["readme.txt"]).unwrap(), b"hello");
}

#[test]
fn a_missing_root_or_a_file_is_no_folder_and_nothing_is_created() {
    let rig = rig();
    let missing = rig._temp.path().join("gone");
    assert!(matches!(
        SharedFolder::at(&missing, None),
        Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
    ));
    assert!(!missing.exists());
    assert!(SharedFolder::at(&rig.root.join("readme.txt"), None).is_err());
    assert!(SharedFolder::at(Path::new("shared"), None).is_err());
}

#[test]
fn a_root_leading_to_a_share_or_a_name_that_is_not_unicode_says_which() {
    use std::os::windows::ffi::OsStringExt;
    let root = Path::new(r"D:\Games");
    assert!(local_root(root, Path::new(r"\\?\D:\Games")).is_ok());
    let share = local_root(root, Path::new(r"\\?\unc\nas\games")).unwrap_err();
    assert_eq!(share.kind(), io::ErrorKind::Unsupported);
    assert!(share.to_string().contains("network share"), "{share}");
    // A lone surrogate, which NTFS allows in a name.
    let mut name: Vec<u16> = r"\\?\D:\".encode_utf16().collect();
    name.push(0xd800);
    let odd = local_root(root, &PathBuf::from(std::ffi::OsString::from_wide(&name))).unwrap_err();
    assert_eq!(odd.kind(), io::ErrorKind::InvalidData);
    assert!(odd.to_string().contains("not valid Unicode"), "{odd}");
    assert!(!odd.to_string().contains("network"), "{odd}");
}

#[test]
fn a_hidden_part_file_of_the_users_survives_an_upload() {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;
    let rig = rig();
    // Hidden as file transfer marks its own, by another program.
    let foreign = rig.root.join("saves").join("foo.part");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .attributes(FILE_ATTRIBUTE_HIDDEN.0)
        .open(&foreign)
        .unwrap()
        .write_all(b"theirs")
        .unwrap();
    assert_ne!(
        fs::metadata(&foreign).unwrap().file_attributes() & FILE_ATTRIBUTE_HIDDEN.0,
        0
    );
    let folder = rig.folder.upload_folder(&["saves"]).unwrap();
    let mut part = rig.folder.create_part(&folder, "slot2.sav").unwrap();
    part.write_all(b"ours").unwrap();
    assert_eq!(part.finish().unwrap(), "slot2.sav");
    drop(folder);
    rig.folder.upload_folder(&["saves"]).unwrap();
    assert_eq!(fs::read(&foreign).unwrap(), b"theirs");
}

#[test]
fn an_upload_lands_in_its_folder_which_stays_put_until_it_ends() {
    let rig = rig();
    let folder = rig.folder.upload_folder(&["saves"]).unwrap();
    let mut part = rig.folder.create_part(&folder, "slot1.sav").unwrap();
    part.write_all(b"new").unwrap();
    // Held open: the folder can't be renamed away under the upload.
    assert!(fs::rename(rig.root.join("saves"), rig.root.join("moved")).is_err());
    drop(folder);
    assert_eq!(part.finish().unwrap(), "slot1 (1).sav");
    assert_eq!(
        fs::read(rig.root.join("saves").join("slot1 (1).sav")).unwrap(),
        b"new"
    );
    assert_eq!(
        fs::read(rig.root.join("saves").join("slot1.sav")).unwrap(),
        b"slot"
    );
    // Released with the upload.
    fs::rename(rig.root.join("saves"), rig.root.join("moved")).unwrap();
    // Uploads take the names file transfer takes.
    let root = rig.folder.upload_folder(&[]).unwrap();
    assert!(matches!(
        rig.folder.create_part(&root, "CON.txt"),
        Err(UserFolderError::InvalidName(_))
    ));
    assert!(matches!(
        rig.folder.create_part(&root, "x.part"),
        Err(UserFolderError::InvalidName(_))
    ));
    assert!(matches!(
        rig.folder.upload_folder(&["readme.txt"]),
        Err(UserFolderError::Io(error)) if error.kind() == io::ErrorKind::NotFound
    ));
}
