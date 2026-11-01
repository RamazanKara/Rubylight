use super::*;

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
fn a_file_is_written_whole_and_removed_below_its_folder() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let path = root.join(r"game profiles\Game\Profile[1].json");
    create_folders(root, path.parent().unwrap()).unwrap();
    // Made once, found the next time.
    create_folders(root, path.parent().unwrap()).unwrap();
    write(root, &path, b"one").unwrap();
    write(root, &path, b"two").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"two");
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["Profile[1].json"]);
    // A temporary file a stopped host left is replaced.
    std::fs::write(path.with_file_name("Profile[1].json.rubylight"), b"half").unwrap();
    write(root, &path, b"three").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"three");
    assert!(remove(root, &path).unwrap());
    assert!(!remove(root, &path).unwrap());
    assert!(!remove(root, &root.join(r"gone\file.json")).unwrap());
    let error = format!(
        "{:#}",
        write(root, &root.join(r"gone\file.json"), b"x").unwrap_err()
    );
    assert!(error.contains("doesn't exist"), "{error}");
    let error = format!(
        "{:#}",
        write(root, Path::new(r"C:\elsewhere\x"), b"x").unwrap_err()
    );
    assert!(error.contains("is not in"), "{error}");
}

#[test]
fn a_link_below_the_folder_is_refused_never_written_through() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let elsewhere = tempfile::tempdir().unwrap();
    // A folder on the way.
    let handlers = root.join("handlers");
    junction(&handlers, elsewhere.path());
    let file = handlers.join("Game.js");
    for error in [
        write(root, &file, b"x").unwrap_err(),
        remove(root, &file).unwrap_err(),
        create_folders(root, &handlers.join("more")).unwrap_err(),
    ] {
        let error = format!("{error:#}");
        assert!(
            error.contains(&format!("{} is a link to elsewhere", handlers.display())),
            "{error}"
        );
    }
    assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    std::fs::remove_dir(&handlers).unwrap();
    // The file's own name, and its temporary file's.
    let list = root.join("userprofile.json");
    for link in [list.clone(), root.join("userprofile.json.rubylight")] {
        junction(&link, elsewhere.path());
        let error = format!("{:#}", write(root, &list, b"x").unwrap_err());
        assert!(
            error.contains(&format!("{} is a link to elsewhere", link.display())),
            "{error}"
        );
        std::fs::remove_dir(&link).unwrap();
    }
    assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    // A folder where the file should be stays.
    std::fs::create_dir(&list).unwrap();
    let error = format!("{:#}", write(root, &list, b"x").unwrap_err());
    assert!(error.contains("is a folder"), "{error}");
    assert!(list.is_dir());
}

#[test]
fn the_folder_itself_may_be_reached_through_a_link() {
    // A user's profile may be moved to another drive behind a junction.
    let real = tempfile::tempdir().unwrap();
    let links = tempfile::tempdir().unwrap();
    let root = links.path().join("Profile");
    junction(&root, real.path());
    let path = root.join(r"NucleusCoop\metaInfo.json");
    create_folders(&root, path.parent().unwrap()).unwrap();
    write(&root, &path, b"{}").unwrap();
    assert_eq!(
        std::fs::read(real.path().join(r"NucleusCoop\metaInfo.json")).unwrap(),
        b"{}"
    );
}

#[test]
fn a_short_name_or_dots_on_the_way_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let error = format!(
        "{:#}",
        write(root, &root.join(r"a\..\b.json"), b"x").unwrap_err()
    );
    assert!(error.contains("goes through . or .."), "{error}");
    let long = root.join("game profiles");
    std::fs::create_dir(&long).unwrap();
    let wide: Vec<u16> = long
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
    let short = PathBuf::from(String::from_utf16_lossy(&short[..len]));
    let Some(name) = short.file_name().filter(|name| *name != "game profiles") else {
        eprintln!("skipped: this volume makes no 8.3 short names");
        return;
    };
    let error = format!(
        "{:#}",
        write(root, &root.join(name).join("x.json"), b"x").unwrap_err()
    );
    assert!(error.contains("through a link"), "{error}");
}
