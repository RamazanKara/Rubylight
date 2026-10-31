use super::{ini::UTF8_BOM, *};
use serde_json::json;

const OPTISCALER: &str = "; OptiScaler settings\r\n[Framerate]\r\n; 0 = no limit\r\nFramerateLimit = 59\r\n\r\n[Upscalers]\r\nDx12Upscaler=fsr31\r\n";

thread_local! {
    /// Makes this thread's swaps fail halfway, as ERROR_UNABLE_TO_MOVE_REPLACEMENT does
    /// (the file gone, its new contents under the temporary name), and the move after.
    pub(super) static HALFWAY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// The tests reach their files as the test process.
fn folders(candidates: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    super::folders(candidates, as_host)
}
fn resolve(path: &str, base: Option<&Path>, folders: &[PathBuf]) -> Result<PathBuf, Unusable> {
    super::resolve(path, base, folders, as_host)
}
fn targets(
    entries: &[Entry],
    fps: Option<u32>,
    base: Option<&Path>,
    folders: &[PathBuf],
    report: &mut Report,
) -> Vec<Target> {
    super::targets(entries, fps, base, folders, as_host, report)
}
fn apply(journal: &Path, session: &str, targets: &[Target]) -> Result<Report> {
    super::apply(journal, session, targets, as_host)
}
fn restore(journal: &Path, scope: Scope<'_>) -> Result<Report> {
    super::restore(journal, scope, as_host)
}
fn no_user(_: &mut dyn FnMut()) -> std::io::Result<()> {
    Err(std::io::Error::other("nobody is signed in"))
}

#[test]
fn values_are_fixed_or_follow_the_stream() {
    let parse = |value: Value| Template::parse(&value);
    assert_eq!(
        parse(json!("{stream_fps}")).unwrap(),
        Template::StreamFps(0)
    );
    assert_eq!(
        parse(json!("{stream_fps-1}")).unwrap(),
        Template::StreamFps(-1)
    );
    assert_eq!(
        parse(json!(" { stream_fps + 2 } ")).unwrap(),
        Template::StreamFps(2)
    );
    assert_eq!(
        parse(json!("{stream_fps+0}")).unwrap(),
        Template::StreamFps(0)
    );
    assert_eq!(parse(json!(59)).unwrap(), Template::Fixed("59".into()));
    assert_eq!(
        parse(json!(" auto ")).unwrap(),
        Template::Fixed("auto".into())
    );
    assert_eq!(parse(json!(true)).unwrap(), Template::Fixed("true".into()));
    for bad in [
        json!("{base_fps}"),
        json!("{stream_fps*2}"),
        json!("{stream_fps-}"),
        json!("{stream_fps-1.5}"),
        json!("{stream_fps-99999}"),
        json!("{stream_fps"),
        json!("x{stream_fps}"),
        json!("two\nlines"),
        json!(null),
        json!([1]),
    ] {
        assert!(parse(bad.clone()).is_err(), "{bad}");
    }
    assert_eq!(Template::StreamFps(-1).resolve(Some(60)).unwrap(), "59");
    assert_eq!(Template::StreamFps(0).resolve(Some(120)).unwrap(), "120");
    assert!(Template::StreamFps(-1).resolve(Some(1)).is_err());
    assert!(Template::StreamFps(0).resolve(None).is_err());
    assert_eq!(
        Template::Fixed("auto".into()).resolve(None).unwrap(),
        "auto"
    );
}

#[test]
fn only_the_value_changes_and_every_other_byte_stays() {
    let bytes = OPTISCALER.as_bytes();
    assert_eq!(
        value_of(bytes, "framerate", "frameratelimit")
            .unwrap()
            .as_deref(),
        Some("59")
    );
    let edited = with_value(bytes, "Framerate", "FramerateLimit", "120")
        .unwrap()
        .unwrap();
    assert_eq!(
        String::from_utf8(edited.clone()).unwrap(),
        OPTISCALER.replace("FramerateLimit = 59", "FramerateLimit = 120")
    );
    // And back to the very bytes it had.
    let back = with_value(&edited, "Framerate", "FramerateLimit", "59")
        .unwrap()
        .unwrap();
    assert_eq!(back, bytes);
}

#[test]
fn a_byte_order_mark_and_utf16_files_are_kept() {
    let mut utf8 = UTF8_BOM.to_vec();
    utf8.extend_from_slice(b"[Framerate]\nFramerateLimit=59\n");
    let edited = with_value(&utf8, "Framerate", "FramerateLimit", "120")
        .unwrap()
        .unwrap();
    assert!(edited.starts_with(UTF8_BOM));
    assert_eq!(&edited[3..], b"[Framerate]\nFramerateLimit=120\n");

    let text = "[Framerate]\r\nFramerateLimit=59\r\nName=Ça\r\n";
    for big_endian in [false, true] {
        let mut wide_bytes = if big_endian {
            vec![0xFE, 0xFF]
        } else {
            vec![0xFF, 0xFE]
        };
        for unit in text.encode_utf16() {
            wide_bytes.extend_from_slice(&if big_endian {
                unit.to_be_bytes()
            } else {
                unit.to_le_bytes()
            });
        }
        assert_eq!(
            value_of(&wide_bytes, "Framerate", "Name")
                .unwrap()
                .as_deref(),
            Some("Ça")
        );
        let edited = with_value(&wide_bytes, "Framerate", "FramerateLimit", "120")
            .unwrap()
            .unwrap();
        let back = with_value(&edited, "Framerate", "FramerateLimit", "59")
            .unwrap()
            .unwrap();
        assert_eq!(back, wide_bytes);
        assert_eq!(
            value_of(&edited, "Framerate", "FramerateLimit")
                .unwrap()
                .as_deref(),
            Some("120")
        );
    }
}

#[test]
fn keys_are_found_as_ini_readers_find_them() {
    let text = b"Global = 1\n[First]\n;Key=commented\n#Key=also\nOther=2\n[ Second ]\n  key   =   a value  \nKey=later duplicate\n[First]\nKey=after\n[Broken\nKey=x\n";
    assert_eq!(value_of(text, "", "global").unwrap().as_deref(), Some("1"));
    assert_eq!(
        value_of(text, "second", "KEY").unwrap().as_deref(),
        Some("a value")
    );
    // The section of a header without a closing ] goes on.
    assert_eq!(
        value_of(text, "first", "key").unwrap().as_deref(),
        Some("after")
    );
    assert_eq!(value_of(text, "Third", "Key").unwrap(), None);
    assert_eq!(value_of(text, "First", "Missing").unwrap(), None);
    assert_eq!(with_value(text, "First", "Missing", "1").unwrap(), None);
    // An empty value takes the new one in its place.
    let edited = with_value(b"[A]\nKey=\nNext=1", "A", "Key", "60")
        .unwrap()
        .unwrap();
    assert_eq!(edited, b"[A]\nKey=60\nNext=1");
    assert!(value_of(b"[A]\nKey=\xff\n", "A", "Key").is_err());
}

#[test]
fn entries_are_checked_when_the_app_is_saved() {
    let entry = |path: &str, section: &str, key: &str, value: Value| json!([{"path": path, "section": section, "key": key, "value": value}]);
    assert!(
        parse(&entry(
            "OptiScaler.ini",
            "Framerate",
            "FramerateLimit",
            json!("{stream_fps-1}")
        ))
        .is_ok()
    );
    assert!(
        parse(&entry(
            r"C:\Games\RDR2\OptiScaler.ini",
            "",
            "Limit",
            json!(60)
        ))
        .is_ok()
    );
    assert!(parse(&json!(null)).unwrap().is_empty());
    for (path, section, key, value) in [
        ("OptiScaler.txt", "A", "B", json!(1)),
        (r"..\OptiScaler.ini", "A", "B", json!(1)),
        (r"sub\..\..\x.ini", "A", "B", json!(1)),
        (r"\\server\share\x.ini", "A", "B", json!(1)),
        (r"\\?\C:\x.ini", "A", "B", json!(1)),
        (r"\\.\C:\x.ini", "A", "B", json!(1)),
        (r"\Games\x.ini", "A", "B", json!(1)),
        (r"C:x.ini", "A", "B", json!(1)),
        (r"x.ini:stream", "A", "B", json!(1)),
        ("", "A", "B", json!(1)),
        ("x.ini", "[A]", "B", json!(1)),
        ("x.ini", "A", "", json!(1)),
        ("x.ini", "A", "B=C", json!(1)),
        ("x.ini", "A", "B", json!("{base_fps}")),
    ] {
        assert!(
            parse(&entry(path, section, key, value.clone())).is_err(),
            "{path} {section} {key} {value}"
        );
    }
    assert!(parse(&json!({"path": "x.ini"})).is_err());
    assert!(parse(&json!([{"path": "x.ini", "key": "B"}])).is_err());
    let many: Vec<Value> = (0..=MAX_ENTRIES)
        .map(|i| json!({"path": "x.ini", "key": format!("k{i}"), "value": 1}))
        .collect();
    assert!(parse(&Value::Array(many)).is_err());
}

/// A game folder with an OptiScaler file, the journal beside it in another folder. The
/// folder and the file are as a user writes them; `resolved` is the file as the host
/// resolves it.
struct Game {
    _root: tempfile::TempDir,
    folder: PathBuf,
    file: PathBuf,
    resolved: PathBuf,
    journal: PathBuf,
}
fn game() -> Game {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("RDR2");
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("OptiScaler.ini");
    std::fs::write(&file, OPTISCALER).unwrap();
    let journal = root.path().join("config").join(JOURNAL_FILE);
    Game {
        resolved: std::fs::canonicalize(&file).unwrap(),
        _root: root,
        file,
        journal,
        folder,
    }
}
fn limit(value: &str) -> Entry {
    Entry {
        path: "OptiScaler.ini".into(),
        section: "Framerate".into(),
        key: "FramerateLimit".into(),
        value: json!(value),
    }
}
fn current(file: &Path) -> String {
    value_of(&std::fs::read(file).unwrap(), "Framerate", "FramerateLimit")
        .unwrap()
        .unwrap()
}

#[test]
fn files_resolve_only_inside_the_apps_folders() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    assert_eq!(folders, [std::fs::canonicalize(&game.folder).unwrap()]);
    let resolve = |path: &str| resolve(path, Some(&game.folder), &folders);
    assert_eq!(resolve("OptiScaler.ini").unwrap(), game.resolved);
    assert_eq!(
        resolve(&game.file.to_string_lossy()).unwrap(),
        game.resolved
    );
    assert!(matches!(resolve("missing.ini"), Err(Unusable::Missing(_))));
    assert!(matches!(
        self::resolve("OptiScaler.ini", None, &folders),
        Err(Unusable::Missing(_))
    ));
    // A file beside the folder, named with its full path, is outside it.
    let outside = game.folder.parent().unwrap().join("outside.ini");
    std::fs::write(&outside, "[A]\nB=1\n").unwrap();
    assert!(matches!(
        resolve(&outside.to_string_lossy()),
        Err(Unusable::Refused(_))
    ));
    assert!(matches!(
        resolve(r"..\outside.ini"),
        Err(Unusable::Refused(_))
    ));
    // A folder named .ini is not a file.
    std::fs::create_dir(game.folder.join("dir.ini")).unwrap();
    assert!(matches!(resolve("dir.ini"), Err(Unusable::Refused(_))));
    let big = game.folder.join("big.ini");
    std::fs::write(&big, vec![b'a'; MAX_FILE_BYTES as usize + 1]).unwrap();
    assert!(matches!(resolve("big.ini"), Err(Unusable::Refused(_))));
    // A drive's root and the Windows folder are never an app's folder.
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    assert!(self::folders([PathBuf::from(r"C:\"), PathBuf::from(&windows)]).is_empty());
    assert!(self::folders([PathBuf::from("relative")]).is_empty());
}

#[test]
#[cfg(windows)]
fn a_junction_out_of_the_folder_is_not_followed() {
    let game = game();
    let elsewhere = game.folder.parent().unwrap().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("secret.ini"), "[A]\nB=1\n").unwrap();
    let link = game.folder.join("link");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&elsewhere)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let folders = folders([game.folder.clone()]);
    assert!(matches!(
        resolve(r"link\secret.ini", Some(&game.folder), &folders),
        Err(Unusable::Refused(_))
    ));
}

#[test]
fn a_stream_sets_the_key_and_its_end_puts_the_original_back() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let mut report = Report::default();
    let targets = targets(
        &[limit("{stream_fps}")],
        Some(120),
        Some(&game.folder),
        &folders,
        &mut report,
    );
    assert!(report.skipped.is_empty(), "{report:?}");
    let report = apply(&game.journal, "phone", &targets).unwrap();
    assert_eq!(report.written.len(), 1, "{report:?}");
    assert_eq!(current(&game.file), "120");
    assert_eq!(
        std::fs::read_to_string(&game.file).unwrap(),
        OPTISCALER.replace("= 59", "= 120")
    );
    // The original is journaled on disk.
    let journal = Journal::load(&game.journal).unwrap();
    assert_eq!(journal.changes.len(), 1);
    assert_eq!(
        (
            journal.changes[0].original.as_str(),
            journal.changes[0].written.as_str()
        ),
        ("59", "120")
    );
    // Another device's session leaves it.
    assert_eq!(
        restore(&game.journal, Scope::Session("tv")).unwrap(),
        Report::default()
    );
    assert_eq!(current(&game.file), "120");
    let report = restore(&game.journal, Scope::Session("phone")).unwrap();
    assert_eq!(report.restored.len(), 1, "{report:?}");
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
}

#[test]
fn a_value_the_user_changed_meanwhile_stays() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("120")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    apply(&game.journal, "phone", &targets).unwrap();
    // The game's overlay saves another limit.
    let saved = OPTISCALER.replace("= 59", "= 90").replace("fsr31", "xess");
    std::fs::write(&game.file, &saved).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert!(
        report.restored.is_empty() && report.kept.len() == 1,
        "{report:?}"
    );
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), saved);
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
}

#[test]
fn a_host_start_after_a_crash_puts_back_what_the_journal_holds() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("{stream_fps-1}")],
        Some(120),
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    apply(&game.journal, "phone", &targets).unwrap();
    assert_eq!(current(&game.file), "119");
    // The host stops without a restore; the next start restores everything journaled.
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.restored.len(), 1);
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    // A crash after the journal and before the write leaves the file as it is.
    let mut journal = Journal::default();
    journal.changes.push(Change {
        session: "phone".into(),
        path: game.resolved.clone(),
        folder: PathBuf::new(),
        section: "Framerate".into(),
        key: "FramerateLimit".into(),
        original: "30".into(),
        written: "120".into(),
    });
    journal.save(&game.journal).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert!(report.restored.is_empty() && report.kept.len() == 1);
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    // A file gone since drops its changes.
    journal.save(&game.journal).unwrap();
    std::fs::remove_file(&game.file).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.skipped.len(), 1);
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    // So does a game folder deleted with it, on a drive that is still there.
    journal.changes[0].path = game
        .resolved
        .with_file_name("Deleted")
        .join("OptiScaler.ini");
    journal.changes[0].folder = game.resolved.with_file_name("Deleted");
    journal.save(&game.journal).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    // And an exe folder deleted with the game around it (`Missing\Game\Binaries\Win64`).
    let win64 = game
        .resolved
        .with_file_name("Missing")
        .join("Game")
        .join("Binaries")
        .join("Win64");
    journal.changes[0].path = win64.join("OptiScaler.ini");
    journal.changes[0].folder = win64;
    journal.save(&game.journal).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    // A drive that is not there (not mounted yet) keeps them for a later restore.
    #[cfg(windows)]
    if let Some(drive) = ('D'..='Z')
        .rev()
        .map(|letter| PathBuf::from(format!(r"\\?\{letter}:\")))
        .find(|drive| !drive.exists())
    {
        journal.changes[0].path = drive.join("Games").join("OptiScaler.ini");
        journal.changes[0].folder = drive.join("Games");
        journal.save(&game.journal).unwrap();
        let report = restore(&game.journal, Scope::All).unwrap();
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(
            Journal::load(&game.journal).unwrap().changes,
            journal.changes
        );
    }
}

#[test]
fn a_file_a_failed_write_left_under_its_temporary_name_goes_back() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("120")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    apply(&game.journal, "phone", &targets).unwrap();
    // The swap failed halfway and the move after it too: only the temporary file is left.
    let temporary = game
        .resolved
        .with_file_name(format!(".OptiScaler.ini.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::rename(&game.resolved, &temporary).unwrap();
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.restored.len(), 1, "{report:?}");
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    assert!(!temporary.exists());
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
}

#[test]
fn a_swap_that_failed_halfway_moves_the_new_file_in_and_no_other_failure_makes_one() {
    let game = game();
    let temporary = game.folder.join(".OptiScaler.ini.halfway.tmp");
    // ERROR_UNABLE_TO_MOVE_REPLACEMENT: the file is gone, its new contents are beside it.
    std::fs::remove_file(&game.file).unwrap();
    std::fs::write(&temporary, "[Framerate]\r\nFramerateLimit=120\r\n").unwrap();
    let halfway = recover(
        &game.file,
        &temporary,
        std::io::Error::from_raw_os_error(1176),
    );
    if cfg!(windows) {
        halfway.unwrap();
        assert_eq!(current(&game.file), "120");
        assert!(!temporary.exists());
    } else {
        assert!(halfway.is_err());
    }
    // A file that turns up meanwhile (the game wrote one) is not overwritten.
    std::fs::write(&game.file, "[Framerate]\r\nFramerateLimit=120\r\n").unwrap();
    std::fs::write(&temporary, "new").unwrap();
    assert!(move_in(&temporary, &game.file).is_err());
    assert_eq!(current(&game.file), "120");
    assert!(
        recover(
            &game.file,
            &temporary,
            std::io::Error::from_raw_os_error(1176)
        )
        .is_err()
    );
    assert_eq!(current(&game.file), "120");
    assert!(!temporary.exists());
    // Any other failure leaves no new file, and no temporary one.
    std::fs::remove_file(&game.file).ok();
    std::fs::write(&temporary, "new").unwrap();
    assert!(recover(&game.file, &temporary, std::io::Error::from_raw_os_error(5)).is_err());
    assert!(!game.file.exists() && !temporary.exists());
    // A file the user deleted while it was written stays deleted.
    #[cfg(windows)]
    assert!(replace(&game.resolved, b"[Framerate]\r\nFramerateLimit=120\r\n").is_err());
    assert!(!game.file.exists());
    assert_eq!(std::fs::read_dir(&game.folder).unwrap().count(), 0);
}

#[test]
#[cfg(windows)]
fn a_write_whose_new_file_is_left_beside_it_stays_journaled_and_goes_back() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("120")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    HALFWAY.set(true);
    let report = apply(&game.journal, "phone", &targets);
    HALFWAY.set(false);
    let report = report.unwrap();
    assert!(
        report.written.is_empty() && report.skipped.len() == 1,
        "{report:?}"
    );
    assert!(!game.file.exists());
    assert_eq!(Journal::load(&game.journal).unwrap().changes.len(), 1);
    let report = restore(&game.journal, Scope::All).unwrap();
    assert_eq!(report.restored.len(), 1, "{report:?}");
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    assert_eq!(std::fs::read_dir(&game.folder).unwrap().count(), 1);
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
}

#[test]
fn a_restore_that_changes_nothing_leaves_the_journal_alone() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("60")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    apply(&game.journal, "phone", &targets).unwrap();
    let mut permissions = std::fs::metadata(&game.journal).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&game.journal, permissions.clone()).unwrap();
    // At the sign-in screen nothing goes back, and the journal is not written again.
    let report = super::restore(&game.journal, Scope::All, no_user).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&game.journal, permissions).unwrap();
    restore(&game.journal, Scope::All).unwrap();
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
}

#[test]
#[cfg(windows)]
fn the_folders_down_to_a_file_are_held_while_it_is_written() {
    let game = game();
    let sub = game.folder.join("Binaries");
    std::fs::create_dir(&sub).unwrap();
    let file = std::fs::canonicalize(&sub).unwrap().join("OptiScaler.ini");
    std::fs::write(&file, OPTISCALER).unwrap();
    let folder = std::fs::canonicalize(&game.folder).unwrap();
    let pins = pin(&folder, &file).unwrap();
    assert_eq!(pins.len(), 2);
    // Neither the app's folder nor the one below it can move, so neither can be swapped.
    assert!(std::fs::rename(&sub, game.folder.join("Moved")).is_err());
    assert!(std::fs::rename(&game.folder, game.folder.with_file_name("Moved")).is_err());
    drop(pins);
    std::fs::rename(&sub, game.folder.join("Moved")).unwrap();
    // A folder swapped for a junction before the write is refused, and the write with it.
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&sub)
        .arg(game.folder.join("Moved"))
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    assert!(linked(&pin(&folder, &file).unwrap_err()));
    let target = Target {
        file: file.clone(),
        folder: folder.clone(),
        section: "Framerate".into(),
        key: "FramerateLimit".into(),
        value: "120".into(),
    };
    let report = apply(&game.journal, "phone", &[target]).unwrap();
    assert!(
        report.written.is_empty() && report.skipped.len() == 1,
        "{report:?}"
    );
    assert_eq!(
        current(&game.folder.join("Moved").join("OptiScaler.ini")),
        "59"
    );

    // A folder above the app's: the held ones keep it from moving, and one that is a
    // junction by the time they are held is refused, though the app's folder opens.
    let library = game.folder.with_file_name("Library");
    std::fs::create_dir_all(library.join("Game")).unwrap();
    std::fs::write(library.join("Game").join("OptiScaler.ini"), OPTISCALER).unwrap();
    let folder = std::fs::canonicalize(library.join("Game")).unwrap();
    let file = folder.join("OptiScaler.ini");
    let pins = pin(&folder, &file).unwrap();
    let elsewhere = game.folder.with_file_name("Elsewhere");
    assert!(std::fs::rename(&library, &elsewhere).is_err());
    drop(pins);
    std::fs::rename(&library, &elsewhere).unwrap();
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&library)
        .arg(&elsewhere)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    assert!(linked(&pin(&folder, &file).unwrap_err()));
}

#[test]
fn a_crash_while_another_devices_key_goes_back_leaves_the_pcs_own_value() {
    let game = game();
    let change = |session: &str, written: &str| Change {
        session: session.into(),
        path: game.resolved.clone(),
        folder: PathBuf::new(),
        section: "Framerate".into(),
        key: "FramerateLimit".into(),
        original: "59".into(),
        written: written.into(),
    };
    // The TV wrote 60 over the PC's 59. The handheld's write put the 59 back and journaled
    // its 120, keeping the TV's entry until the file changed. In either order, a restore
    // after a crash before the swap (the file at 60) or after it (at 120) gives the 59.
    for order in [["tv", "handheld"], ["handheld", "tv"]] {
        for on_disk in ["60", "120"] {
            let journal = Journal {
                version: 1,
                changes: order
                    .iter()
                    .map(|session| change(session, if *session == "tv" { "60" } else { "120" }))
                    .collect(),
            };
            journal.save(&game.journal).unwrap();
            std::fs::write(
                &game.file,
                OPTISCALER.replace("= 59", &format!("= {on_disk}")),
            )
            .unwrap();
            restore(&game.journal, Scope::All).unwrap();
            assert_eq!(current(&game.file), "59", "{order:?} {on_disk}");
        }
    }
}

#[test]
fn an_original_goes_back_whatever_its_length() {
    let game = game();
    let long = "x".repeat(MAX_TEXT + 10);
    std::fs::write(
        &game.file,
        format!("[Framerate]\r\nFramerateLimit={long}\r\n"),
    )
    .unwrap();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("60")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    apply(&game.journal, "phone", &targets).unwrap();
    assert_eq!(current(&game.file), "60");
    restore(&game.journal, Scope::All).unwrap();
    assert_eq!(current(&game.file), long);
}

#[test]
fn without_the_windows_user_nothing_is_written_and_restores_wait() {
    let game = game();
    assert!(super::folders([game.folder.clone()], no_user).is_empty());
    let folders = folders([game.folder.clone()]);
    assert!(matches!(
        super::resolve("OptiScaler.ini", Some(&game.folder), &folders, no_user),
        Err(Unusable::Missing(_))
    ));
    let targets = targets(
        &[limit("60")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    let report = super::apply(&game.journal, "phone", &targets, no_user).unwrap();
    assert!(
        report.written.is_empty() && report.skipped.len() == 1,
        "{report:?}"
    );
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    apply(&game.journal, "phone", &targets).unwrap();
    let report = super::restore(&game.journal, Scope::All, no_user).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(current(&game.file), "60");
    restore(&game.journal, Scope::All).unwrap();
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
}

#[test]
fn a_restore_can_spare_the_sessions_still_running() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let mut upscaler = limit("xess");
    upscaler.section = "Upscalers".into();
    upscaler.key = "Dx12Upscaler".into();
    for (session, entry) in [("tv", limit("60")), ("phone", upscaler)] {
        let targets = targets(
            &[entry],
            None,
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        );
        apply(&game.journal, session, &targets).unwrap();
    }
    let running = BTreeSet::from(["tv".to_owned()]);
    restore(&game.journal, Scope::Except(&running)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&game.file).unwrap(),
        OPTISCALER.replace("= 59", "= 60")
    );
    restore(&game.journal, Scope::Except(&BTreeSet::new())).unwrap();
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
}

#[test]
fn a_missing_key_is_skipped_and_the_others_are_written() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let mut report = Report::default();
    let mut missing = limit("1");
    missing.key = "NotThere".into();
    let mut gone = limit("1");
    gone.path = "gone.ini".into();
    let mut upscaler = limit("xess");
    upscaler.section = "Upscalers".into();
    upscaler.key = "Dx12Upscaler".into();
    let targets = targets(
        &[missing, gone, limit("{stream_fps}"), upscaler],
        None,
        Some(&game.folder),
        &folders,
        &mut report,
    );
    // The missing file, and the rate the stream did not state, are skipped at once.
    assert_eq!(report.skipped.len(), 2, "{report:?}");
    assert_eq!(targets.len(), 2);
    let report = apply(&game.journal, "phone", &targets).unwrap();
    assert_eq!(report.skipped.len(), 1, "{report:?}");
    assert_eq!(report.written.len(), 1, "{report:?}");
    let text = std::fs::read_to_string(&game.file).unwrap();
    assert_eq!(text, OPTISCALER.replace("fsr31", "xess"));
    assert!(!text.contains("NotThere"));
}

#[test]
fn another_devices_change_goes_back_before_the_next_write() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let at = |fps| {
        targets(
            &[limit("{stream_fps}")],
            Some(fps),
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        )
    };
    apply(&game.journal, "tv", &at(60)).unwrap();
    assert_eq!(current(&game.file), "60");
    let report = apply(&game.journal, "handheld", &at(120)).unwrap();
    assert_eq!(
        (report.restored.len(), report.written.len()),
        (1, 1),
        "{report:?}"
    );
    assert_eq!(current(&game.file), "120");
    let journal = Journal::load(&game.journal).unwrap();
    assert_eq!(journal.changes.len(), 1);
    assert_eq!(journal.changes[0].session, "handheld");
    // The original is the PC's own value, not the TV's.
    assert_eq!(journal.changes[0].original, "59");
    restore(&game.journal, Scope::Session("tv")).unwrap();
    assert_eq!(current(&game.file), "120");
    restore(&game.journal, Scope::Session("handheld")).unwrap();
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
}

#[test]
fn writing_again_in_one_session_keeps_the_first_original() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    let at = |fps| {
        targets(
            &[limit("{stream_fps}")],
            Some(fps),
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        )
    };
    apply(&game.journal, "phone", &at(60)).unwrap();
    apply(&game.journal, "phone", &at(90)).unwrap();
    let journal = Journal::load(&game.journal).unwrap();
    assert_eq!(
        (
            journal.changes[0].original.as_str(),
            journal.changes[0].written.as_str()
        ),
        ("59", "90")
    );
    // A value the user set in between becomes the one to go back to.
    std::fs::write(&game.file, OPTISCALER.replace("= 59", "= 75")).unwrap();
    apply(&game.journal, "phone", &at(120)).unwrap();
    let journal = Journal::load(&game.journal).unwrap();
    assert_eq!(journal.changes[0].original, "75");
    restore(&game.journal, Scope::All).unwrap();
    assert_eq!(current(&game.file), "75");
}

#[test]
fn an_unreadable_journal_writes_nothing() {
    let game = game();
    std::fs::create_dir_all(game.journal.parent().unwrap()).unwrap();
    std::fs::write(&game.journal, "{not json").unwrap();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("120")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    assert!(apply(&game.journal, "phone", &targets).is_err());
    assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    assert!(restore(&game.journal, Scope::All).is_err());
}

#[test]
#[cfg(windows)]
fn a_read_only_file_is_not_overwritten_and_nothing_stays_journaled() {
    let game = game();
    let mut permissions = std::fs::metadata(&game.file).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&game.file, permissions.clone()).unwrap();
    let folders = folders([game.folder.clone()]);
    let targets = targets(
        &[limit("120")],
        None,
        Some(&game.folder),
        &folders,
        &mut Report::default(),
    );
    let report = apply(&game.journal, "phone", &targets).unwrap();
    assert!(
        report.written.is_empty() && report.skipped.len() == 1,
        "{report:?}"
    );
    assert_eq!(current(&game.file), "59");
    assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    // No temporary file is left beside it.
    assert_eq!(std::fs::read_dir(&game.folder).unwrap().count(), 1);
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&game.file, permissions).unwrap();
}

#[test]
fn frame_limit_keys_that_disagree_with_the_limiter_are_named() {
    let target = |key: &str, value: &str| Target {
        file: PathBuf::from("x.ini"),
        folder: PathBuf::new(),
        section: String::new(),
        key: key.into(),
        value: value.into(),
    };
    let targets = [
        target("FramerateLimit", "59"),
        target("FPSCap", "120"),
        target("MaxFrameLimit", "0"),
        target("FrameLimit", "auto"),
        target("Upscaler", "59"),
        target("fps", "119.6"),
    ];
    let named: Vec<&str> = limit_conflicts(&targets, 120)
        .map(|t| t.key.as_str())
        .collect();
    assert_eq!(named, ["FramerateLimit"]);
}
