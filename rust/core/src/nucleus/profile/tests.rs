use super::*;

#[test]
fn pads_are_named_by_xinput_index_like_nucleus() {
    assert_eq!(gamepad_guid(0), "00000000-0000-0000-0000-200000000001");
    assert_eq!(gamepad_guid(3), "00000000-0000-0000-0000-200000000004");
    assert_eq!(gamepad_guid(9), "00000000-0000-0000-0000-200000000010");
}

const SCREEN: Rect = Rect {
    x: 0,
    y: 0,
    width: 2560,
    height: 1440,
};

#[test]
fn the_profile_has_one_xinput_player_per_slot_and_starts_by_itself() {
    let players = [
        Player {
            xinput: 1,
            bounds: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        },
        Player {
            xinput: 2,
            bounds: Rect {
                x: 100,
                y: 100,
                width: 1280,
                height: 720,
            },
        },
    ];
    let p = build(&players, &[SCREEN], false);
    assert_eq!(p["Title"], TITLE);
    assert_eq!(p["AutoPlay"]["Enabled"], false);
    assert_eq!(
        build(&players, &[SCREEN], true)["AutoPlay"]["Enabled"],
        true
    );
    assert_eq!(p["Use XInput Index"], true);
    assert_eq!(p["Player(s)"], 2);
    assert_eq!(p["UseSplitDiv"]["Enabled"], false);
    assert_eq!(p["AutoDesktopScaling"]["Enabled"], false);
    let data = p["Data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[1]["PlayerID"], 1);
    assert_eq!(
        data[1]["GamepadGuid"],
        "00000000-0000-0000-0000-200000000003"
    );
    assert_eq!(data[1]["IsXInput"], true);
    assert_eq!(data[1]["IsKeyboardPlayer"], false);
    assert_eq!(data[1]["MonitorBounds"]["Width"], 1280);
    assert_eq!(data[1]["ScreenIndex"], 0);
    assert_eq!(data[1]["Owner"]["Display"]["Width"], 2560);
    assert_eq!(data[1]["Guests"]["GUIDS"], "");
    assert_eq!(p["AllScreens"][0]["Height"], 1440);
    // Every key Nucleus 2.4 reads without a null check is there.
    for key in [
        "Options",
        "WindowsSetupTiming",
        "PauseBetweenInstanceLaunch",
        "UseNicknames",
        "CutscenesModeSettings",
        "Network",
        "CustomLayout",
        "Notes",
        "AudioSettings",
        "AudioInstances",
    ] {
        assert!(!p[key].is_null(), "{key}");
    }
}

/// #59: Nucleus 2.4.2's `GenericGameHandler.Play` runs `int.Parse` on a
/// player's ideal processor unless it is "*", and an empty one failed
/// every start with a FormatException once the game's process was up.
#[test]
fn each_player_s_processor_settings_are_ones_nucleus_starts_with() {
    let player = Player {
        xinput: 0,
        bounds: SCREEN,
    };
    let p = build(&[player, player], &[SCREEN], false);
    for data in p["Data"].as_array().unwrap() {
        let processor = &data["Processor"];
        let ideal = processor["IdealProcessor"].as_str().unwrap();
        assert!(
            ideal == "*" || ideal.parse::<i32>().is_ok(),
            "Nucleus can't parse IdealProcessor {ideal:?}"
        );
        assert_eq!(ideal, "*");
        // No affinity and Normal priority: Nucleus keeps its own.
        assert_eq!(processor["ProcessorAffinity"], "");
        assert_eq!(processor["ProcessorPriorityClass"], "Normal");
    }
}

#[test]
fn the_host_keeps_one_profile_in_nucleus_numbering() {
    assert_eq!(number(&[]), 1);
    let others = [(1, None), (2, Some("Couch".to_string()))];
    assert_eq!(number(&others), 3);
    let ours = [(1, None), (2, Some(TITLE.to_string())), (3, None)];
    assert_eq!(number(&ours), 2);
    assert_eq!(file_number("Profile[12].json"), Some(12));
    assert_eq!(file_number("Profile[x].json"), None);
    assert_eq!(file_number("notes.txt"), None);
}

/// The value of `key` in `section` as text.
fn value(text: &[u8], section: &str, key: &str) -> Option<String> {
    ini_value(text, section, key).map(|v| String::from_utf8(v).unwrap())
}

/// Nucleus 2.4.2's own file; a checkout may have turned its line ends
/// into CRLF.
fn shipped() -> String {
    DEFAULT_SETTINGS.replace("\r\n", "\n")
}

#[test]
fn settings_ini_forces_the_hosts_keys_and_keeps_every_other_byte() {
    let ini = "[Misc]\r\nDisableGameProfiles=True\r\n[Dev]\r\nUseXinputIndex=False\r\nOther=1\r\n";
    let settings = settings_ini(ini.as_bytes());
    let text = &settings.text;
    assert_eq!(value(text, "Dev", "UseXinputIndex").unwrap(), "True");
    // Profiles switched off would make Nucleus ignore the host's.
    assert_eq!(value(text, "Misc", "DisableGameProfiles").unwrap(), "False");
    assert_eq!(value(text, "Dev", "DisablePathCheck").unwrap(), "True");
    assert_eq!(value(text, "Dev", "ShowFavoriteOnly").unwrap(), "False");
    assert_eq!(
        value(text, "Misc", "IgnoreInputLockReminder").unwrap(),
        "True"
    );
    assert!(
        text.starts_with(b"[Misc]\r\nDisableGameProfiles=False\r\n"),
        "{}",
        String::from_utf8_lossy(text)
    );
    assert!(String::from_utf8_lossy(text).contains("\r\nOther=1\r\n"));
    // Pads by index and profiles on stay set; the rest goes back.
    let keys: Vec<&str> = settings.forced.iter().map(|f| f.key.as_str()).collect();
    // A key completed from Nucleus' defaults that already has the
    // session's value is no change.
    assert_eq!(keys, ["DisablePathCheck"]);
    assert_eq!(settings.forced[0].before, b"False");
    assert_eq!(settings.forced[0].forced, b"True");
}

#[test]
fn a_partial_empty_or_missing_settings_file_is_completed_so_every_key_parses() {
    // The host wrote this two-key file where none was: Nucleus then
    // stopped on bool.Parse("") with a message box (#59).
    let old = b"[Dev]\r\nUseXinputIndex=True\r\n[Misc]\r\nDisableGameProfiles=False\r\n";
    let defaults = Ini::parse(DEFAULT_SETTINGS.as_bytes()).entries();
    assert!(defaults.len() > 100, "{}", defaults.len());
    for text in [&old[..], b"", b"\n"] {
        let settings = settings_ini(text);
        for (section, key, default) in &defaults {
            let found = ini_value(&settings.text, section, key)
                .unwrap_or_else(|| panic!("{section}.{key} missing"));
            let parses =
                |v: &[u8]| v.eq_ignore_ascii_case(b"true") || v.eq_ignore_ascii_case(b"false");
            if parses(default.as_slice()) {
                assert!(parses(found.as_slice()), "{section}.{key}={found:?}");
            }
        }
        assert!(settings.added.contains(&"Dev.Blur".to_string()));
        assert!(settings.added.contains(&"Hotkeys.Close".to_string()));
        // Completed, it stays as it is.
        let again = settings_ini(&settings.text);
        assert_eq!(again.text, settings.text);
        assert!(again.added.is_empty() && again.forced.is_empty());
    }
    let settings = settings_ini(old);
    assert!(!settings.added.contains(&"Dev.UseXinputIndex".to_string()));
    assert!(
        settings
            .text
            .starts_with(b"[Dev]\r\nUseXinputIndex=True\r\n")
    );
}

#[test]
fn a_value_in_the_pcs_code_page_its_bom_and_line_ends_are_kept() {
    // Nucleus writes through kernel32's ANSI calls: ü is 0xFC in Latin-1.
    let mut ini = b"\xef\xbb\xbf[Misc]\nSettingsLocation=C:\\M\xfcnchen\n".to_vec();
    ini.extend_from_slice(shipped().as_bytes());
    let settings = settings_ini(&ini);
    assert!(
        settings
            .text
            .starts_with(b"\xef\xbb\xbf[Misc]\nSettingsLocation=C:\\M\xfcnchen\n")
    );
    // The file's LF stays LF, also for what it changes.
    assert!(!settings.text.contains(&b'\r'));
    assert_eq!(
        ini_value(&settings.text, "Misc", "SettingsLocation").unwrap(),
        b"C:\\M\xfcnchen"
    );
    assert_eq!(
        value(&settings.text, "Dev", "DisablePathCheck").unwrap(),
        "True"
    );
    // A last line without an end gets one before a new line follows.
    let text = ini_set(b"[Dev]\r\nA=1", &[("Dev", "B", "2")]);
    assert_eq!(text, b"[Dev]\r\nA=1\r\nB=2\r\n");
}

#[test]
fn ini_values_are_read_and_set_as_nucleus_reads_them() {
    let ini = b"[misc]\r\n GameSortingOpt = Sort_1 \r\n[Dev]\r\nShowFavoriteOnly=True\r\n";
    assert_eq!(value(ini, "Misc", "gamesortingopt").unwrap(), "Sort_1");
    assert_eq!(ini_value(ini, "Dev", "Missing"), None);
    assert_eq!(ini_value(ini, "Audio", "Missing"), None);
    assert_eq!(
        ini_set(
            ini,
            &[("Dev", "ShowFavoriteOnly", "False"), ("Audio", "On", "1")]
        ),
        b"[misc]\r\n GameSortingOpt = Sort_1 \r\n[Dev]\r\nShowFavoriteOnly=False\r\n[Audio]\r\nOn=1\r\n"
    );
}

/// The shipped file with a user's settings that stalled a background
/// stream at Nucleus' start (#59): its folder check on (a message box
/// for a Documents folder in OneDrive or a folder under Program
/// Files), favourites only, and only games for a single keyboard listed.
fn seeded() -> String {
    let seeded = shipped()
        .replace("ShowFavoriteOnly=False", "ShowFavoriteOnly=True")
        .replace("GameSortingOpt=\n", "GameSortingOpt=Sort_2,Show_Type2\n");
    assert!(seeded.contains("DisablePathCheck=False\n"), "{seeded}");
    assert!(seeded.contains("ShowFavoriteOnly=True\n"), "{seeded}");
    assert!(
        seeded.contains("GameSortingOpt=Sort_2,Show_Type2\n"),
        "{seeded}"
    );
    seeded
}

#[test]
fn settings_ini_lets_nucleus_start_and_list_the_game_without_a_message_box() {
    let seeded = seeded();
    let settings = settings_ini(seeded.as_bytes());
    assert!(settings.added.is_empty(), "{:?}", settings.added);
    let fixed = String::from_utf8(settings.text.clone()).unwrap();
    let get = |section: &str, key: &str| value(fixed.as_bytes(), section, key).unwrap();
    assert_eq!(get("Dev", "DisablePathCheck"), "True");
    assert_eq!(get("Dev", "ShowFavoriteOnly"), "False");
    assert_eq!(get("Misc", "GameSortingOpt"), "Sort_2");
    assert_eq!(get("Dev", "UseXinputIndex"), "True");
    // Every other line is as shipped, in its place.
    let changed = [
        "DisablePathCheck",
        "ShowFavoriteOnly",
        "GameSortingOpt",
        "UseXinputIndex",
    ];
    let before: Vec<&str> = seeded.split('\n').collect();
    let after: Vec<&str> = fixed.split('\n').collect();
    assert_eq!(before.len(), after.len());
    for (old, new) in before.iter().zip(&after) {
        let key = old.split('=').next().unwrap();
        if changed.contains(&key) {
            assert!(new.starts_with(&format!("{key}=")), "{old} became {new}");
        } else {
            assert_eq!(old, new);
        }
    }
    // A second pass changes nothing.
    let again = settings_ini(&settings.text);
    assert_eq!(again.text, settings.text);
    assert!(again.forced.is_empty());
    // The Sort_* order is kept, the filters go, an empty value stays.
    for (sorting, kept) in [
        ("Show_Type1,Sort_4,Show_Type3", "Sort_4"),
        ("Show_Type1", ""),
        ("Sort_1", "Sort_1"),
        ("", ""),
    ] {
        let text = settings_ini(format!("[Misc]\r\nGameSortingOpt={sorting}\r\n").as_bytes());
        assert_eq!(
            value(&text.text, "Misc", "GameSortingOpt").unwrap(),
            kept,
            "{sorting}"
        );
    }
}

#[test]
fn the_users_values_come_back_where_the_session_s_still_are() {
    let seeded = seeded();
    let settings = settings_ini(seeded.as_bytes());
    let keys: Vec<&str> = settings.forced.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(
        keys,
        ["DisablePathCheck", "ShowFavoriteOnly", "GameSortingOpt"]
    );
    // Put back whole; pads by index stay.
    let (back, changed) = put_back_settings(&settings.text, &settings.forced);
    assert_eq!(changed, 3);
    assert_eq!(
        String::from_utf8(back).unwrap(),
        seeded.replace("UseXinputIndex=False", "UseXinputIndex=True")
    );
    // Nucleus or the user changed the list's order meanwhile: it stays.
    let changed_since = ini_set(&settings.text, &[("Misc", "GameSortingOpt", "Sort_3")]);
    let (back, changed) = put_back_settings(&changed_since, &settings.forced);
    assert_eq!(changed, 2);
    assert_eq!(value(&back, "Misc", "GameSortingOpt").unwrap(), "Sort_3");
    assert_eq!(value(&back, "Dev", "ShowFavoriteOnly").unwrap(), "True");
    // Again, nothing is left to put back.
    assert_eq!(put_back_settings(&back, &settings.forced).1, 0);
}

#[test]
fn a_list_filter_beside_spaced_entries_comes_back() {
    // Nucleus writes no spaces, but a hand-edited file may have them:
    // the session's value is trimmed as it is read back.
    let users = b"[Misc]\r\nGameSortingOpt=Show_Type1, Sort_2 ,Sort_4\r\n";
    let settings = settings_ini(users);
    assert_eq!(
        value(&settings.text, "Misc", "GameSortingOpt").unwrap(),
        "Sort_2,Sort_4"
    );
    let (back, changed) = put_back_settings(&settings.text, &settings.forced);
    assert_eq!(changed, settings.forced.len());
    assert_eq!(
        value(&back, "Misc", "GameSortingOpt").unwrap(),
        "Show_Type1, Sort_2 ,Sort_4"
    );
    // Spaced entries without a filter are no change of the session's.
    let unfiltered = settings_ini(b"[Misc]\r\nGameSortingOpt=Sort_2, Sort_4\r\n");
    assert!(
        unfiltered.forced.iter().all(|f| f.key != "GameSortingOpt"),
        "{:?}",
        unfiltered.forced
    );
    assert_eq!(
        value(&unfiltered.text, "Misc", "GameSortingOpt").unwrap(),
        "Sort_2, Sort_4"
    );
}

#[test]
fn a_new_copys_settings_are_nucleus_own_with_the_sessions_values() {
    let text = default_settings_ini();
    // Every key of the shipped file, nothing to complete or force again.
    for (section, key, _) in Ini::parse(DEFAULT_SETTINGS.as_bytes()).entries() {
        assert!(
            ini_value(&text, &section, &key).is_some(),
            "{section}.{key}"
        );
    }
    let again = settings_ini(&text);
    assert!(again.added.is_empty() && again.forced.is_empty());
    for (section, key, forced) in FORCED {
        assert_eq!(value(&text, section, key).unwrap(), forced);
    }
    // As the release ships it: nobody's own values.
    assert_eq!(
        value(&text, "Misc", "NucleusAccountPassword").unwrap(),
        "12345"
    );
}

/// An entry as Nucleus 2.4.2 saves the first game it loads without a
/// metaInfo.json (`GameMetaInfo.SaveGameMetaInfo`): every value false.
const SAVED_FIRST: &str = r#"{
  "Rubylight-sandfall_win64_shipping-049e4c28": {
    "TotalPlayTime": null,
    "LastPlayedAt": null,
    "IconPath": null,
    "SaveProfile": false,
    "DisableProfiles": false,
    "KeepSymLink": false,
    "Favorite": false,
    "FirstLaunch": false,
    "CheckUpdate": false,
    "UseApiIndex": false,
    "UseApiIndexForGuests": false,
    "ProfileAssignGamepadByButonPress": false,
    "SteamLanguage": "App Setting"
  },
  "Other": {
    "UseApiIndex": false,
    "Favorite": true
  }
}"#;

fn entry(text: &[u8], guid: &str) -> serde_json::Map<String, Value> {
    let games: Value = serde_json::from_slice(text).unwrap();
    games[guid].as_object().unwrap().clone()
}

#[test]
fn a_game_saved_with_pads_by_button_press_gets_them_by_index() {
    let guid = "Rubylight-sandfall_win64_shipping-049e4c28";
    let text = meta_info_by_index(Some(SAVED_FIRST.as_bytes()), guid)
        .unwrap()
        .unwrap();
    let game = entry(&text, guid);
    assert_eq!(game["UseApiIndex"], true);
    assert_eq!(game["UseApiIndexForGuests"], true);
    // Every other value stays, the other game's too.
    let before = entry(SAVED_FIRST.as_bytes(), guid);
    assert_eq!(game.len(), before.len());
    for (key, value) in &before {
        if !key.starts_with("UseApiIndex") {
            assert_eq!(&game[key], value, "{key}");
        }
    }
    assert_eq!(
        entry(&text, "Other"),
        entry(SAVED_FIRST.as_bytes(), "Other")
    );
    // Written once: the file it wrote needs nothing more.
    assert_eq!(meta_info_by_index(Some(&text), guid), Ok(None));
}

#[test]
fn without_metainfo_nucleus_gets_an_empty_list_to_seed_from_settings() {
    // Not Nucleus' own first save, which turns pads by index off.
    assert_eq!(meta_info_by_index(None, "Game"), Ok(Some(b"{}".to_vec())));
    assert_eq!(
        meta_info_by_index(Some(b" \r\n"), "Game"),
        Ok(Some(b"{}".to_vec()))
    );
    assert_eq!(
        meta_info_by_index(Some(b"\xef\xbb\xbf"), "Game"),
        Ok(Some(b"{}".to_vec()))
    );
}

#[test]
fn a_game_with_pads_by_index_or_no_entry_leaves_the_file_alone() {
    let by_index = br#"{"Game": {"UseApiIndex": true, "UseApiIndexForGuests": true}}"#;
    assert_eq!(meta_info_by_index(Some(by_index), "Game"), Ok(None));
    // Missing values are Settings.ini's, which the session sets.
    assert_eq!(
        meta_info_by_index(Some(br#"{"Game": {"Favorite": true}}"#), "Game"),
        Ok(None)
    );
    // A game Nucleus has not saved yet takes Settings.ini's value.
    assert_eq!(
        meta_info_by_index(Some(SAVED_FIRST.as_bytes()), "Game"),
        Ok(None)
    );
    // Its byte order mark is read past.
    let mut marked = b"\xef\xbb\xbf".to_vec();
    marked.extend_from_slice(by_index);
    assert_eq!(meta_info_by_index(Some(&marked), "Game"), Ok(None));
}

#[test]
fn a_metainfo_nucleus_can_t_read_is_refused() {
    for text in [&b"{"[..], b"[]", b"null", br#"{"Game": 1}"#] {
        let error = meta_info_by_index(Some(text), "Game").unwrap_err();
        assert!(error.starts_with("Nucleus Co-op can't read"), "{error}");
    }
}
