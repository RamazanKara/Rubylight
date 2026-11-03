use super::*;
use serde_json::json;

/// A config folder and a game folder of their own, removed afterwards.
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rl-game-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("game")).unwrap();
        Self(root)
    }
    fn store(&self) -> Store {
        Store::new(&self.0.join("config"), game_files::as_host)
    }
    fn file(&self, name: &str, text: &str) -> (String, PathBuf) {
        let path = self.0.join("game").join(name);
        std::fs::write(&path, text).unwrap();
        (format!("%GAME%\\{name}"), path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

const NONE: &BTreeSet<String> = &BTreeSet::new();

#[test]
fn a_first_launch_keeps_the_file_and_the_end_saves_it_as_the_devices_copy() {
    let s = Scratch::new();
    let store = s.store();
    let (written, path) = s.file("Settings.ini", "res=4k");
    let files = [(written.clone(), path.clone())];
    let begun = store
        .begin("app", "Game", "tv", "TV", &files, NONE)
        .unwrap();
    assert_eq!(begun.report.first, std::slice::from_ref(&written));
    assert_eq!(text(&path), "res=4k", "nothing to put in yet");
    std::fs::write(&path, "res=1080p").unwrap();
    let report = store.finish(&begun.id).unwrap();
    assert_eq!(report.saved, std::slice::from_ref(&written));
    assert_eq!(report.restored, std::slice::from_ref(&written));
    assert_eq!(text(&path), "res=4k", "the PC's own file is back");
    assert_eq!(
        text(&store.device_copy("app", "tv", &file_key(&written))),
        "res=1080p"
    );
    assert!(store.saved("app", "tv", &written));
    assert!(!store.saved("app", "deck", &written));
    assert!(store.journal().unwrap().swaps.is_empty());
}

#[test]
fn each_device_gets_its_own_copy_and_the_pc_keeps_its_file() {
    let s = Scratch::new();
    let store = s.store();
    let (written, path) = s.file("Settings.ini", "pc");
    let files = [(written.clone(), path.clone())];
    for (device, value) in [("tv", "tv-1"), ("deck", "deck-1")] {
        let begun = store
            .begin("app", "Game", device, device, &files, NONE)
            .unwrap();
        std::fs::write(&path, value).unwrap();
        store.finish(&begun.id).unwrap();
    }
    std::fs::write(&path, "pc-2").unwrap();
    let begun = store
        .begin("app", "Game", "tv", "TV", &files, NONE)
        .unwrap();
    assert_eq!(begun.report.placed, std::slice::from_ref(&written));
    assert_eq!(text(&path), "tv-1", "the TV's copy is in place");
    std::fs::write(&path, "tv-2").unwrap();
    store.finish(&begun.id).unwrap();
    assert_eq!(
        text(&path),
        "pc-2",
        "a change made at the PC meanwhile stays the PC's"
    );
    let copy = store.device_copy("app", "tv", &file_key(&written));
    assert_eq!(text(&copy), "tv-2");
    let mut previous = copy.into_os_string();
    previous.push(".previous");
    assert_eq!(
        text(Path::new(&previous)),
        "tv-1",
        "the copy before is kept"
    );
    assert_eq!(
        text(&store.device_copy("app", "deck", &file_key(&written))),
        "deck-1"
    );
    assert_eq!(text(&store.original("app", &file_key(&written))), "pc-2");
}

#[test]
fn a_crash_while_playing_is_finished_at_the_next_start() {
    let s = Scratch::new();
    let (written, path) = s.file("Settings.ini", "pc");
    let files = [(written.clone(), path.clone())];
    let id = s
        .store()
        .begin("app", "Game", "tv", "TV", &files, NONE)
        .unwrap()
        .id;
    std::fs::write(&path, "tv").unwrap();
    // The host is gone; a new one starts with a store of its own.
    let store = s.store();
    let live = BTreeSet::from([id.clone()]);
    assert!(
        store.recover(&live).unwrap().is_empty(),
        "a running app's swap is left alone"
    );
    let results = store.recover(NONE).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].1.is_ok());
    assert_eq!(text(&path), "pc");
    assert_eq!(
        text(&store.device_copy("app", "tv", &file_key(&written))),
        "tv"
    );
    assert!(store.journal().unwrap().swaps.is_empty());
}

/// A crash between two steps: the journal says where each file was.
#[test]
fn a_crash_between_steps_never_saves_the_original_as_a_devices_copy() {
    let s = Scratch::new();
    let store = s.store();
    let (written, path) = s.file("Settings.ini", "pc");
    let files = [(written.clone(), path.clone())];
    let copy = store.device_copy("app", "tv", &file_key(&written));
    state::atomic_write(&copy, b"tv").unwrap();
    for step in [Step::Placing, Step::Saved] {
        let id = store
            .begin("app", "Game", "tv", "TV", &files, NONE)
            .unwrap()
            .id;
        let mut journal = store.journal().unwrap();
        journal.swaps[0].files[0].step = step;
        store.save(&journal).unwrap();
        std::fs::write(&path, "half").unwrap();
        let results = store.recover(NONE).unwrap();
        assert_eq!(results[0].0.id, id);
        assert!(results[0].1.is_ok());
        assert_eq!(text(&path), "pc", "{step:?}: the original is back");
        assert_eq!(text(&copy), "tv", "{step:?}: the copy is not touched");
    }
}

#[test]
fn a_missing_file_fails_the_launch_and_changes_nothing() {
    let s = Scratch::new();
    let store = s.store();
    let (present, present_path) = s.file("Present.ini", "pc");
    let missing = "%GAME%\\Missing.ini".to_owned();
    let missing_path = s.0.join("game").join("Missing.ini");
    let error = store
        .begin(
            "app",
            "Game",
            "tv",
            "TV",
            &[(present, present_path.clone()), (missing, missing_path)],
            NONE,
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("does not exist"), "{error:#}");
    assert!(store.journal().unwrap().swaps.is_empty());
    assert_eq!(text(&present_path), "pc");
}

#[test]
fn a_file_the_game_removed_is_put_back_and_said_loudly() {
    let s = Scratch::new();
    let store = s.store();
    let (written, path) = s.file("Settings.ini", "pc");
    let begun = store
        .begin(
            "app",
            "Game",
            "tv",
            "TV",
            &[(written.clone(), path.clone())],
            NONE,
        )
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    let report = store.finish(&begun.id).unwrap();
    assert_eq!(report.warnings.len(), 1, "{report:?}");
    assert!(report.saved.is_empty());
    assert_eq!(text(&path), "pc");
    assert!(!store.saved("app", "tv", &written));
}

#[test]
fn a_copy_that_cannot_go_in_puts_back_the_files_already_swapped() {
    let s = Scratch::new();
    let store = s.store();
    let (first, first_path) = s.file("A.ini", "pc-a");
    let (second, second_path) = s.file("B.ini", "pc-b");
    state::atomic_write(&store.device_copy("app", "tv", &file_key(&first)), b"tv-a").unwrap();
    // A folder where the second copy should be: it cannot be read.
    std::fs::create_dir_all(store.device_copy("app", "tv", &file_key(&second))).unwrap();
    let error = store
        .begin(
            "app",
            "Game",
            "tv",
            "TV",
            &[
                (first.clone(), first_path.clone()),
                (second, second_path.clone()),
            ],
            NONE,
        )
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("reading the copy"),
        "{error:#}"
    );
    assert_eq!(text(&first_path), "pc-a");
    assert_eq!(text(&second_path), "pc-b");
    assert_eq!(
        text(&store.device_copy("app", "tv", &file_key(&first))),
        "tv-a",
        "the copy is not overwritten by the original"
    );
    assert!(store.journal().unwrap().swaps.is_empty());
}

/// Two devices at once: one swap holds a file. A second launch of it fails while the first app
/// runs, and finishes the first swap when it no longer does.
#[test]
fn a_second_launch_waits_for_the_first_swap_of_the_same_file() {
    let s = Scratch::new();
    let store = s.store();
    let (written, path) = s.file("Settings.ini", "pc");
    let files = [(written.clone(), path.clone())];
    let tv = store
        .begin("app", "Game", "tv", "TV", &files, NONE)
        .unwrap();
    std::fs::write(&path, "tv").unwrap();
    let live = BTreeSet::from([tv.id.clone()]);
    let error = store
        .begin("other", "Game (HDR)", "deck", "Deck", &files, &live)
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("Game from TV, which is still running"),
        "{error:#}"
    );
    assert_eq!(text(&path), "tv", "the running game's file stays");
    // The TV's app stopped without its swap finishing (a host crash): the Deck's launch finishes it.
    let deck = store
        .begin("other", "Game (HDR)", "deck", "Deck", &files, NONE)
        .unwrap();
    assert_eq!(
        text(&store.device_copy("app", "tv", &file_key(&written))),
        "tv"
    );
    assert_eq!(text(&path), "pc", "the Deck has no copy yet: the PC's file");
    store.finish(&deck.id).unwrap();
    assert!(store.journal().unwrap().swaps.is_empty());
}

#[cfg(windows)]
#[test]
fn a_locked_file_fails_loudly_and_stays_journaled_until_it_can_go_back() {
    use std::os::windows::fs::OpenOptionsExt;
    let s = Scratch::new();
    let store = s.store();
    let (written, path) = s.file("Settings.ini", "pc");
    let files = [(written.clone(), path.clone())];
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let error = store
        .begin("app", "Game", "tv", "TV", &files, NONE)
        .unwrap_err();
    assert!(format!("{error:#}").contains("in use"), "{error:#}");
    drop(held);
    let begun = store
        .begin("app", "Game", "tv", "TV", &files, NONE)
        .unwrap();
    std::fs::write(&path, "tv").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let error = store.finish(&begun.id).unwrap_err();
    assert!(format!("{error:#}").contains("in use"), "{error:#}");
    let journal = store.journal().unwrap();
    assert_eq!(journal.swaps[0].files[0].step, Step::Playing);
    assert!(journal.swaps[0].error.is_some());
    drop(held);
    assert!(store.recover(NONE).unwrap()[0].1.is_ok());
    assert_eq!(text(&path), "pc");
    assert_eq!(
        text(&store.device_copy("app", "tv", &file_key(&written))),
        "tv"
    );
}

#[test]
fn an_unreadable_journal_swaps_nothing() {
    let s = Scratch::new();
    let store = s.store();
    let (written, path) = s.file("Settings.ini", "pc");
    state::atomic_write(
        &s.0.join("config").join(FOLDER).join(JOURNAL_FILE),
        b"{oops",
    )
    .unwrap();
    assert!(
        store
            .begin("app", "Game", "tv", "TV", &[(written, path)], NONE)
            .is_err()
    );
    assert!(store.recover(NONE).is_err());
}

#[test]
fn paths_are_checked_and_expanded_from_the_users_environment() {
    let extra = |list: Value| BTreeMap::from([(SETTING.to_owned(), list)]);
    assert!(paths(&BTreeMap::new()).unwrap().is_empty());
    assert_eq!(
        paths(&extra(json!([
            "  %LOCALAPPDATA%\\Game\\Settings.ini ",
            "",
            "D:/Games/x.cfg"
        ])))
        .unwrap(),
        ["%LOCALAPPDATA%\\Game\\Settings.ini", "D:/Games/x.cfg"]
    );
    for bad in [
        json!("C:\\x.ini"),
        json!([1]),
        json!(["C:\\a.ini", "c:/A.INI"]),
    ] {
        assert!(paths(&extra(bad.clone())).is_err(), "{bad}");
    }
    for bad in [
        "\\\\server\\share\\x.ini",
        "relative\\x.ini",
        "C:\\Games\\..\\Windows\\x.ini",
        "C:\\a:b.ini",
        "%LOCALAPPDATA\\x.ini",
        "%%\\x.ini",
        "C:\\Games\\",
        "C:\\x\n.ini",
    ] {
        assert!(check_path(bad).is_err(), "{bad}");
    }
    let lookup = |name: &str| match name.to_ascii_uppercase().as_str() {
        "LOCALAPPDATA" => Some("C:\\Users\\me\\AppData\\Local\\".to_owned()),
        "BAD" => Some("..".to_owned()),
        _ => None,
    };
    assert_eq!(
        expand("%localappdata%/Game/Settings.ini", lookup).unwrap(),
        PathBuf::from("C:\\Users\\me\\AppData\\Local\\Game\\Settings.ini")
    );
    assert!(format!("{:#}", expand("%NOPE%\\x.ini", lookup).unwrap_err()).contains("not set"),);
    assert!(expand("%BAD%\\x.ini", lookup).is_err());
}

#[test]
fn store_names_stay_inside_the_store() {
    assert_eq!(store_name("0f3a-uuid"), "0f3a-uuid");
    for name in ["..", "a\\b", "", "C:x"] {
        let stored = store_name(name);
        assert!(
            !stored.contains(['\\', '/', ':']) && stored != ".." && !stored.starts_with('.'),
            "{stored}"
        );
    }
    assert_ne!(
        file_key("C:\\A\\Settings.ini"),
        file_key("C:\\B\\Settings.ini")
    );
    assert_eq!(
        file_key("C:\\A\\Settings.ini"),
        file_key("c:/a/settings.INI")
    );
}
