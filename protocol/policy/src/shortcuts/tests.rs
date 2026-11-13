use super::*;

#[test]
fn a_shortcut_moves_within_the_list_and_a_gone_one_is_an_error() {
    let text = r#"{"version":1,"data":[{"id":"a","name":"A","keys":["WIN","TAB"]},{"id":"b","name":"B","keys":["WIN","D"]},{"id":"c","name":"C","keys":["WIN","G"]}]}"#;
    let mut shortcuts = Shortcuts::parse(text).unwrap();
    let order = |s: &Shortcuts| s.data.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>().join("");
    shortcuts.move_to("c", 0).unwrap();
    assert_eq!(order(&shortcuts), "cab");
    shortcuts.move_to("c", 99).unwrap();
    assert_eq!(order(&shortcuts), "abc");
    assert!(shortcuts.move_to("gone", 0).is_err());
    assert_eq!(order(&shortcuts), "abc");
}

#[test]
fn android_document_round_trips_and_invalid_chords_do_not_replace_entries() {
    let text = r#"{"version":1,"data":[{"id":"desktop","name":"Task view","keys":["WIN","TAB"]}]}"#;
    let mut shortcuts = Shortcuts::parse(text).unwrap();
    assert_eq!(Shortcuts::parse(&shortcuts.encode()).unwrap(), shortcuts);
    let before = shortcuts.clone();
    assert_eq!(shortcuts.save_chord(Some("desktop"), "Bad", "CTRL + CTRL_LEFT", "x"), Err(ShortcutError::RepeatedKey("CTRL_LEFT".into())));
    assert_eq!(shortcuts, before);
    assert_eq!(Shortcuts::parse(&text.replace("\"TAB\"", "\"UNKNOWN\"")), Err(ShortcutError::UnsupportedKey("UNKNOWN".into())));
    assert_eq!(Shortcuts::parse(&text.replace("\"version\":1", "\"version\":2")), Err(ShortcutError::Version));
    let entry = serde_json::to_string(&shortcuts.data[0]).unwrap();
    assert_eq!(Shortcuts::parse(&format!(r#"{{"data":[{entry},{entry}]}}"#)), Err(ShortcutError::BadId));
    assert_eq!(key_code("KEYCODE_F24"), Some(0x87));
    assert_eq!(key_code("VK_MENU"), Some(0xA4));
    assert_eq!(key_code("NUMPAD_9"), Some(0x69));
    assert_eq!(key_code("PRINT"), Some(0x2A));
    assert_eq!(key_code("SCREENSHOT"), Some(0x2C));
    shortcuts.save_chord(None, "Portable aliases", "VK_LCONTROL + PRINTSCREEN + F01", "desktop").unwrap();
    assert_eq!(shortcuts.data[1].keys, ["CTRL", "SYSRQ", "F1"]);
    assert_eq!(shortcuts.data[1].id, "desktop-1");
    for code in (0x30..=0x39).chain(0x41..=0x5A).chain(0x60..=0x87) {
        assert_eq!(key_code(&key_name(code)), Some(code));
    }
}

#[test]
fn a_shortcut_added_in_the_stream_gets_a_free_id_and_the_limit_holds() {
    let mut shortcuts = Shortcuts::default();
    let keys = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(shortcuts.save(None, " Task view ", &keys(&["WIN", "TAB"]), "android").unwrap(), "android-1");
    assert_eq!(shortcuts.data[0].name, "Task view");
    assert_eq!(shortcuts.save(None, "Snip", &keys(&["WIN", "SHIFT", "S"]), "android").unwrap(), "android-2");
    shortcuts.remove("android-1").unwrap();
    assert_eq!(shortcuts.save(None, "Again", &keys(&["F5"]), "android").unwrap(), "android-1");
    assert_eq!(shortcuts.remove("android-9"), Err(ShortcutError::Gone));
    assert_eq!(shortcuts.save(Some("gone"), "x", &keys(&["A"]), "android"), Err(ShortcutError::Gone));
    assert_eq!(shortcuts.save(None, "", &keys(&["A"]), "android"), Err(ShortcutError::BadName));
    assert_eq!(shortcuts.save(None, "x", &[], "android"), Err(ShortcutError::ChordLength));
    while shortcuts.data.len() < MAX_SHORTCUTS {
        let n = shortcuts.data.len();
        shortcuts.save(None, &format!("s{n}"), &keys(&["F1"]), "android").unwrap();
    }
    let full = shortcuts.clone();
    assert_eq!(shortcuts.save(None, "one more", &keys(&["F2"]), "android"), Err(ShortcutError::TooMany));
    assert_eq!(shortcuts, full);
    assert_eq!(Shortcuts::parse(&shortcuts.encode()).unwrap(), shortcuts);
}

#[test]
fn a_saved_chord_replaces_the_built_in_with_the_same_keys() {
    assert_eq!(built_ins(&[]).len(), BUILT_INS.len());
    let saved = [Shortcut { id: "a".into(), name: "Taskmgr".into(), keys: vec!["ESC".into(), "SHIFT".into(), "CTRL".into()] }];
    let ids: Vec<_> = built_ins(&saved).iter().map(|b| b.id).collect();
    assert_eq!(ids, ["switch_window", "close_window", "start_menu", "show_desktop", "game_bar", "record_that"]);
    for built_in in BUILT_INS {
        let keys: Vec<String> = built_in.keys.iter().map(|k| k.to_string()).collect();
        assert_eq!(normalize_keys(&keys).unwrap(), keys, "{}", built_in.id);
    }
    let ids: Vec<_> = BUILT_INS.iter().map(|b| b.id).collect();
    assert_eq!(ids, ["task_manager", "switch_window", "close_window", "start_menu", "show_desktop", "game_bar", "record_that"]);
    let keys = |id: &str| BUILT_INS.iter().find(|b| b.id == id).unwrap().keys;
    assert_eq!(keys("game_bar"), ["WIN", "G"]);
    assert_eq!(keys("record_that"), ["WIN", "ALT", "G"]);
    assert_eq!(chord_label(&keys("record_that").iter().map(|k| k.to_string()).collect::<Vec<_>>()), "Win + Alt + G");
}

/// Android's editor hands over keys in the order they were tapped: Esc, Ctrl, Shift sent Esc alone,
/// then Ctrl + Shift. Both clients send what this returns.
#[test]
fn modifiers_go_first_whatever_order_the_keys_were_picked_in() {
    let keys = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(normalize_keys(&keys(&["ESC", "CTRL", "SHIFT"])).unwrap(), ["CTRL", "SHIFT", "ESC"]);
    assert_eq!(normalize_keys(&keys(&["F4", "ALT"])).unwrap(), ["ALT", "F4"]);
    // Stable: modifiers keep their own order, and so do the other keys.
    assert_eq!(normalize_keys(&keys(&["S", "SHIFT", "WIN"])).unwrap(), ["SHIFT", "WIN", "S"]);
    assert_eq!(normalize_keys(&keys(&["A", "CTRL", "B"])).unwrap(), ["CTRL", "A", "B"]);
    let mut shortcuts = Shortcuts::default();
    shortcuts.save(None, "Task Manager", &keys(&["ESC", "CTRL", "SHIFT"]), "android").unwrap();
    assert_eq!(shortcuts.data[0].keys, ["CTRL", "SHIFT", "ESC"]);
    let file = r#"{"version":1,"data":[{"id":"a","name":"Old","keys":["ESC","SHIFT","CTRL"]}]}"#;
    assert_eq!(Shortcuts::parse(file).unwrap().data[0].keys, ["SHIFT", "CTRL", "ESC"]);
}

/// An old Android shortcut can hold a key without a portable name (a media key); it is no built-in.
#[test]
fn a_saved_chord_with_an_unknown_key_does_not_hide_a_built_in() {
    let saved = [Shortcut { id: "a".into(), name: "Old".into(), keys: vec!["CTRL".into(), "SHIFT".into(), "ESC".into(), "MEDIA_PLAY_PAUSE".into()] }];
    assert_eq!(built_ins(&saved).len(), BUILT_INS.len());
    let saved = [Shortcut { id: "a".into(), name: "Old".into(), keys: vec!["ANDROID_KEYCODE_300".into()] }];
    assert_eq!(built_ins(&saved).len(), BUILT_INS.len());
    assert_eq!(normalize_keys(&["CTRL".into(), "ANDROID_KEYCODE_300".into()]), Err(ShortcutError::UnsupportedKey("ANDROID_KEYCODE_300".into())));
}

#[test]
fn every_picker_key_is_a_saved_name_with_a_label() {
    let mut seen = HashSet::new();
    for key in PICKER {
        assert!(seen.insert(*key), "{key} twice");
        let code = key_code(key).unwrap_or_else(|| panic!("{key} unsupported"));
        assert_eq!(key_name(code), *key);
        assert!(!key_label(key).is_empty());
    }
    assert!(PICKER[..4].iter().all(|k| is_modifier(k)));
    assert!(!PICKER[4..].iter().any(|k| is_modifier(k)));
    assert_eq!(chord_label(&["CTRL".into(), "SHIFT".into(), "ESC".into()]), "Ctrl + Shift + Esc");
    assert_eq!(chord_label(&["WIN".into(), "DPAD_LEFT".into()]), "Win + ←");
    assert_eq!(chord_label(&["CAPS_LOCK".into(), "SHIFT".into()]), "Shift + Caps Lock");
    assert_eq!(chord_label(&["ESC".into(), "SHIFT".into(), "CTRL".into()]), "Shift + Ctrl + Esc");
    assert_eq!(key_label("NUMPAD_4"), "Num 4");
}

fn keys(k: &[&str]) -> Vec<String> {
    k.iter().map(|s| s.to_string()).collect()
}

const CTRL_ALT_SHIFT: u8 = 0x07;

/// Every action has a default in saved form, IDs are unique, and no client has a chord twice,
/// old chords included.
#[test]
fn every_hotkey_has_a_default_and_no_client_binds_a_chord_twice() {
    let mut ids = HashSet::new();
    for hotkey in HOTKEYS {
        assert!(ids.insert(hotkey.id), "{} twice", hotkey.id);
        assert_eq!(hotkey_keys(&keys(hotkey.keys)).unwrap(), keys(hotkey.keys), "{}", hotkey.id);
        for (_, old) in hotkey.old {
            assert_eq!(hotkey_keys(&keys(old)).unwrap(), keys(old), "{}", hotkey.id);
        }
    }
    for client in [Client::Desktop, Client::Android] {
        let list = bound(client, &HotkeyBindings::new());
        let chords: Vec<(u8, u16)> = list.iter().flat_map(|b| b.keys.iter().chain(&b.old)).map(|k| hotkey_code(k)).collect();
        for (n, chord) in chords.iter().enumerate() {
            assert!(!chords[n + 1..].contains(chord), "{chord:?} twice on {client:?}");
        }
        assert!(list.iter().all(|b| !b.changed && b.keys.is_some()));
    }
    // The shared actions exist on both clients, each platform's own on its client only.
    let ids = |client| hotkeys(client).map(|h| h.id).collect::<Vec<_>>();
    assert_eq!(ids(Client::Desktop), ["stream_menu", "help", "quit", "capture", "mouse_mode", "overlay", "send_clipboard", "fetch_clipboard", "fullscreen", "minimize"]);
    assert_eq!(ids(Client::Android), ["stream_menu", "quit", "capture", "overlay", "pc_keyboard", "cursor"]);
}

/// Moonlight's meanings win: M switches the mouse on the desktop; Android's old M still opens
/// its stream menu, and its old C still shows the cursor, with Tab and P the new defaults.
#[test]
fn old_android_chords_still_work_there_and_give_way_to_a_new_chord() {
    let none = HotkeyBindings::new();
    let android = bound(Client::Android, &none);
    assert_eq!(hotkey_at(&android, CTRL_ALT_SHIFT, 0x09), Some(("stream_menu", false)));
    assert_eq!(hotkey_at(&android, CTRL_ALT_SHIFT, u16::from(b'M')), Some(("stream_menu", true)));
    assert_eq!(hotkey_at(&android, CTRL_ALT_SHIFT, u16::from(b'C')), Some(("cursor", true)));
    assert_eq!(hotkey_at(&android, CTRL_ALT_SHIFT, u16::from(b'P')), Some(("cursor", false)));
    let desktop = bound(Client::Desktop, &none);
    assert_eq!(hotkey_at(&desktop, CTRL_ALT_SHIFT, u16::from(b'M')), Some(("mouse_mode", false)));
    assert_eq!(hotkey_at(&desktop, CTRL_ALT_SHIFT, u16::from(b'C')), Some(("fetch_clipboard", false)));
    // Exactly the chord's modifiers: Ctrl+Alt alone, or with Windows held too, reaches the PC.
    assert_eq!(hotkey_at(&desktop, 0x06, u16::from(b'Q')), None);
    assert_eq!(hotkey_at(&desktop, 0x0F, u16::from(b'Q')), None);
    // A chord of the user's takes an old one; the old one then stops.
    let m = keys(&["CTRL", "ALT", "SHIFT", "M"]);
    assert_eq!(check_hotkey(Client::Android, "overlay", &m, &none), Ok(m.clone()));
    let bindings = HotkeyBindings::from([("overlay".to_string(), binding_text(Some(&m)))]);
    let android = bound(Client::Android, &bindings);
    assert_eq!(hotkey_at(&android, CTRL_ALT_SHIFT, u16::from(b'M')), Some(("overlay", false)));
    assert!(android.iter().find(|b| b.id == "stream_menu").unwrap().old.is_empty());
    // A changed action has no old chord.
    let bindings = HotkeyBindings::from([("cursor".to_string(), "CTRL+ALT+SHIFT+L".to_string())]);
    assert_eq!(hotkey_at(&bound(Client::Android, &bindings), CTRL_ALT_SHIFT, u16::from(b'C')), None);
}

/// A changed chord does the action and the default stops; Off does nothing; a taken chord is
/// refused naming the action that has it.
#[test]
fn a_rebind_moves_the_action_off_stops_it_and_a_taken_chord_is_refused() {
    let mut bindings = HotkeyBindings::new();
    let f1 = check_hotkey(Client::Desktop, "quit", &keys(&["F1", "CTRL_RIGHT"]), &bindings).unwrap();
    assert_eq!(f1, ["CTRL", "F1"]);
    bindings.insert("quit".into(), binding_text(Some(&f1)));
    bindings.insert("overlay".into(), binding_text(None));
    let desktop = bound(Client::Desktop, &bindings);
    assert_eq!(hotkey_at(&desktop, 0x02, 0x70), Some(("quit", false)));
    assert_eq!(hotkey_at(&desktop, CTRL_ALT_SHIFT, u16::from(b'Q')), None);
    assert_eq!(hotkey_at(&desktop, CTRL_ALT_SHIFT, u16::from(b'S')), None, "off");
    let overlay = desktop.iter().find(|b| b.id == "overlay").unwrap();
    assert_eq!((overlay.keys.clone(), overlay.changed), (None, true));
    assert_eq!(check_hotkey(Client::Desktop, "capture", &keys(&["CTRL", "F1"]), &bindings), Err(HotkeyError::Taken("quit")));
    assert_eq!(check_hotkey(Client::Desktop, "quit", &keys(&["CTRL", "F1"]), &bindings), Ok(f1.clone()), "its own chord again");
    // The freed default can go to another action.
    let q = keys(&["SHIFT", "CTRL", "ALT", "Q"]);
    assert_eq!(check_hotkey(Client::Desktop, "capture", &q, &bindings), Ok(keys(&["CTRL", "ALT", "SHIFT", "Q"])));
    // The default again counts as unchanged.
    bindings.insert("quit".into(), "CTRL+ALT+SHIFT+Q".into());
    assert!(!bound(Client::Desktop, &bindings).iter().find(|b| b.id == "quit").unwrap().changed);
    // A hand-edited binding that does not read is the default.
    bindings.insert("quit".into(), "CTRL+NOPE".into());
    assert_eq!(hotkey_at(&bound(Client::Desktop, &bindings), CTRL_ALT_SHIFT, u16::from(b'Q')), Some(("quit", false)));
}

#[test]
fn a_hotkey_chord_has_two_to_four_keys_a_modifier_and_one_other_key() {
    let none = HotkeyBindings::new();
    let check = |k: &[&str]| check_hotkey(Client::Desktop, "quit", &keys(k), &none);
    assert_eq!(check(&["CTRL"]), Err(HotkeyError::Length));
    assert_eq!(check(&["CTRL", "ALT", "SHIFT", "Q", "W"]), Err(HotkeyError::Length));
    assert_eq!(check(&["Q", "W"]), Err(HotkeyError::OneKey));
    assert_eq!(check(&["CTRL", "SHIFT"]), Err(HotkeyError::OneKey));
    assert_eq!(check(&["CTRL", "Q", "W"]), Err(HotkeyError::OneKey));
    assert_eq!(check(&["WIN", "Q"]), Err(HotkeyError::Modifier));
    assert_eq!(check(&["CTRL", "CTRL_RIGHT", "Q"]), Err(HotkeyError::RepeatedKey("CTRL_RIGHT".into())));
    assert_eq!(check(&["CTRL", "ALT", "DELETE"]), Err(HotkeyError::Reserved));
    assert_eq!(check(&["CTRL", "BOGUS"]), Err(HotkeyError::UnsupportedKey("BOGUS".into())));
    assert_eq!(check(&["ALT", "F4"]), Ok(keys(&["ALT", "F4"])));
    assert_eq!(check_hotkey(Client::Desktop, "cursor", &keys(&["CTRL", "Q"]), &none), Err(HotkeyError::NotHere));
    assert_eq!(check_hotkey(Client::Android, "teleport", &keys(&["CTRL", "Q"]), &none), Err(HotkeyError::UnknownAction("teleport".into())));
    assert_eq!(HotkeyError::Taken("quit").detail(), Some("quit"));
    assert_eq!(binding_text(Some(&keys(&["CTRL", "F1"]))), "CTRL+F1");
}

/// A key pressed alone keeps Nightjar's prefix; a modifier alone or a key without a name does not.
#[test]
fn a_lone_key_becomes_ctrl_alt_shift_and_that_key() {
    let none = HotkeyBindings::new();
    let check = |id: &str, k: &[&str]| check_hotkey(Client::Desktop, id, &keys(k), &none);
    assert_eq!(check("capture", &["F9"]), Ok(keys(&["CTRL", "ALT", "SHIFT", "F9"])));
    assert_eq!(check("quit", &["Q"]), Ok(keys(&["CTRL", "ALT", "SHIFT", "Q"])), "its own default");
    assert_eq!(check("capture", &["Q"]), Err(HotkeyError::Taken("quit")));
    assert_eq!(check("capture", &["SHIFT"]), Err(HotkeyError::Length));
    assert_eq!(check("capture", &["WIN"]), Err(HotkeyError::Length));
    assert_eq!(check("capture", &["BOGUS"]), Err(HotkeyError::UnsupportedKey("BOGUS".into())));
    assert_eq!(check_hotkey(Client::Android, "cursor", &keys(&["F2"]), &none), Ok(keys(&["CTRL", "ALT", "SHIFT", "F2"])));
}

/// A hotkey never reaches the PC, so it must not be a chord people type or edit with, nor one
/// Windows' shell keeps.
#[test]
fn chords_that_type_edit_or_belong_to_windows_are_refused() {
    let none = HotkeyBindings::new();
    let check = |k: &[&str]| check_hotkey(Client::Desktop, "capture", &keys(k), &none);
    for typing in [
        &["SHIFT", "A"][..], &["SHIFT", "SPACE"], &["SHIFT", "TAB"], &["SHIFT", "DPAD_LEFT"], &["SHIFT", "1"],
        &["SHIFT", "HOME"], &["SHIFT", "MINUS"], &["SHIFT", "NUMPAD_4"], &["SHIFT", "ENTER"],
        &["CTRL", "C"], &["CTRL", "V"], &["CTRL", "X"], &["CTRL", "Z"], &["CTRL", "A"], &["CTRL", "Y"],
        &["CTRL", "BACKSPACE"], &["CTRL", "DPAD_RIGHT"], &["CTRL", "SHIFT", "Z"], &["CTRL", "SHIFT", "DPAD_LEFT"],
        // AltGr: Ctrl+Alt+Q is German @ on Windows, (right) Alt+Q on Android.
        &["CTRL", "ALT", "Q"], &["ALT", "Q"], &["ALT", "7"], &["CTRL", "ALT", "LEFT_BRACKET"], &["ALT", "SPACE"],
        // AltGr+Shift on Android: Polish Ż, Nordic and Canadian Multilingual capitals.
        &["ALT", "SHIFT", "Q"], &["ALT", "SHIFT", "Z"], &["ALT", "SHIFT", "MINUS"], &["ALT", "SHIFT", "4"],
    ] {
        assert_eq!(check(typing), Err(HotkeyError::Typing), "{typing:?}");
    }
    for shell in [
        &["ALT", "TAB"][..], &["ALT", "SHIFT", "TAB"], &["CTRL", "ALT", "TAB"], &["ALT", "ESC"], &["ALT", "SHIFT", "ESC"],
        &["CTRL", "ESC"], &["CTRL", "SHIFT", "ESC"], &["CTRL", "ALT", "DELETE"], &["CTRL", "ALT", "SHIFT", "DELETE"],
    ] {
        assert_eq!(check(shell), Err(HotkeyError::Reserved), "{shell:?}");
    }
    for fine in [
        &["SHIFT", "F5"][..], &["SHIFT", "ESC"], &["CTRL", "F1"], &["CTRL", "TAB"], &["CTRL", "SHIFT", "A"], &["ALT", "F4"],
        &["ALT", "SHIFT", "F9"], &["CTRL", "ALT", "F9"], &["CTRL", "ALT", "SHIFT", "1"], &["CTRL", "ALT", "SHIFT", "ESC"],
    ] {
        assert!(check(fine).is_ok(), "{fine:?}: {:?}", check(fine));
    }
    // Every default and old chord stays allowed.
    for hotkey in HOTKEYS {
        assert!(hotkey_keys(&keys(hotkey.keys)).is_ok(), "{}", hotkey.id);
        for (_, old) in hotkey.old {
            assert!(hotkey_keys(&keys(old)).is_ok(), "{} old", hotkey.id);
        }
    }
}

/// Reset brings back a default another action may have taken since; then it is refused naming
/// that action, as a change would be.
#[test]
fn a_reset_to_a_chord_another_action_has_is_refused() {
    let mut bindings = HotkeyBindings::new();
    // Disconnect to Ctrl+F9, Release input to Disconnect's old Ctrl+Alt+Shift+Q, then reset Disconnect.
    bindings.insert("quit".into(), "CTRL+F9".into());
    let q = check_hotkey(Client::Desktop, "capture", &keys(&["CTRL", "ALT", "SHIFT", "Q"]), &bindings).unwrap();
    bindings.insert("capture".into(), binding_text(Some(&q)));
    assert_eq!(check_reset(Client::Desktop, "quit", &bindings), Err(HotkeyError::Taken("capture")));
    // The stream menu off, Disconnect to its Ctrl+Alt+Shift+Tab, then reset the stream menu.
    let mut bindings = HotkeyBindings::from([("stream_menu".to_string(), OFF.to_string())]);
    let tab = check_hotkey(Client::Android, "quit", &keys(&["CTRL", "ALT", "SHIFT", "TAB"]), &bindings).unwrap();
    bindings.insert("quit".into(), binding_text(Some(&tab)));
    assert_eq!(check_reset(Client::Android, "stream_menu", &bindings), Err(HotkeyError::Taken("quit")));
    assert_eq!(check_reset(Client::Android, "cursor", &bindings), Ok(()));
    bindings.remove("quit");
    assert_eq!(check_reset(Client::Android, "stream_menu", &bindings), Ok(()));
    assert_eq!(check_reset(Client::Android, "teleport", &bindings), Err(HotkeyError::UnknownAction("teleport".into())));
}

/// Two actions saved with one chord (a hand-edited file): the earlier in the list has it, the
/// later has none and names the one that does.
#[test]
fn a_repeated_chord_leaves_the_later_action_without_one() {
    let bindings = HotkeyBindings::from([("capture".to_string(), "CTRL+ALT+SHIFT+Q".to_string())]);
    let desktop = bound(Client::Desktop, &bindings);
    let capture = desktop.iter().find(|b| b.id == "capture").unwrap();
    assert_eq!((capture.keys.clone(), capture.taken), (None, Some("quit")));
    assert_eq!(hotkey_at(&desktop, CTRL_ALT_SHIFT, u16::from(b'Q')), Some(("quit", false)));
    assert_eq!(hotkey_at(&desktop, CTRL_ALT_SHIFT, u16::from(b'Z')), None);
    assert!(desktop.iter().filter(|b| b.id != "capture").all(|b| b.taken.is_none() && b.keys.is_some()));
}

/// A captured desktop stream keeps a keyboard way out: the stream menu's chord or Disconnect's.
/// Android always has Back.
#[test]
fn the_desktop_keeps_the_stream_menu_or_disconnect() {
    let mut bindings = HotkeyBindings::new();
    assert_eq!(check_off(Client::Desktop, "stream_menu", &bindings), Ok(()));
    bindings.insert("stream_menu".into(), OFF.into());
    assert_eq!(check_off(Client::Desktop, "overlay", &bindings), Ok(()));
    assert_eq!(check_off(Client::Desktop, "quit", &bindings), Err(HotkeyError::LastWayOut));
    assert_eq!(check_off(Client::Android, "quit", &bindings), Ok(()));
    assert_eq!(check_off(Client::Desktop, "cursor", &bindings), Err(HotkeyError::NotHere));
    bindings.insert("stream_menu".into(), "CTRL+F2".into());
    assert_eq!(check_off(Client::Desktop, "quit", &bindings), Ok(()));
}

/// A hand-edited desktop file that leaves no way out: both off, or Disconnect on Help's chord.
/// The stream menu gets its default back, from the action that has it if one does; Android
/// keeps the file as it is (it has Back).
#[test]
fn a_file_without_a_way_out_gives_the_stream_menu_its_default_back() {
    let tab = keys(&["CTRL", "ALT", "SHIFT", "TAB"]);
    let menu = |list: &[Bound]| list.iter().find(|b| b.id == "stream_menu").unwrap().clone();
    let both_off = HotkeyBindings::from([("stream_menu".to_string(), OFF.to_string()), ("quit".to_string(), OFF.to_string())]);
    let desktop = bound(Client::Desktop, &both_off);
    assert_eq!((menu(&desktop).keys, menu(&desktop).changed), (Some(tab.clone()), false));
    assert_eq!(hotkey_at(&desktop, CTRL_ALT_SHIFT, 0x09), Some(("stream_menu", false)));
    assert_eq!(desktop.iter().find(|b| b.id == "quit").unwrap().keys, None);
    assert_eq!(menu(&bound(Client::Android, &both_off)).keys, None);
    let on_help = HotkeyBindings::from([("stream_menu".to_string(), OFF.to_string()), ("quit".to_string(), "CTRL+ALT+SHIFT+H".to_string())]);
    let desktop = bound(Client::Desktop, &on_help);
    assert_eq!(menu(&desktop).keys, Some(tab.clone()));
    assert_eq!(desktop.iter().find(|b| b.id == "quit").unwrap().taken, Some("help"));
    // The default taken by another action: the way out wins and that action is left without.
    let mut taken = on_help.clone();
    taken.insert("overlay".into(), binding_text(Some(&tab)));
    let desktop = bound(Client::Desktop, &taken);
    assert_eq!(hotkey_at(&desktop, CTRL_ALT_SHIFT, 0x09), Some(("stream_menu", false)));
    let overlay = desktop.iter().find(|b| b.id == "overlay").unwrap();
    assert_eq!((overlay.keys.clone(), overlay.taken), (None, Some("stream_menu")));
    // With a way out the file is kept as it is, and turning the last one off is still refused.
    let menu_off = HotkeyBindings::from([("stream_menu".to_string(), OFF.to_string())]);
    assert_eq!(menu(&bound(Client::Desktop, &menu_off)).keys, None);
    assert_eq!(check_off(Client::Desktop, "quit", &menu_off), Err(HotkeyError::LastWayOut));
}
