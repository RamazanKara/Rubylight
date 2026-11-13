use super::*;

/// Android's setting lists every choice, in the menu's order, under the tile's own name.
#[test]
fn androids_button_setting_offers_every_assignable_tile() {
    let xml = include_str!("../../../../app/src/main/res/values/stream_menu_buttons.xml");
    let items = |array: &str| -> Vec<String> {
        let body = xml.split(&format!("name=\"{array}\"")).nth(1).unwrap().split("</string-array>").next().unwrap();
        body.split("<item>").skip(1).map(|item| item.split("</item>").next().unwrap().to_string()).collect()
    };
    let tiles: Vec<&Tile> = TILES.iter().filter(|tile| assignable(tile)).filter(|tile| tile.id != "rate_change").collect();
    let mut values = vec!["default".to_string(), NO_TILE.to_string()];
    values.extend(tiles.iter().map(|tile| tile.id.to_string()));
    assert_eq!(items("stream_menu_button_values"), values);
    let mut names = vec!["@string/stream_menu_button_default".to_string(), "@string/stream_menu_button_none".to_string()];
    names.extend(tiles.iter().map(|tile| format!("@string/stream_tile_{}", tile.label.unwrap().key)));
    assert_eq!(items("stream_menu_button_names"), names);
}

fn ids(layout: &Layout) -> Vec<String> {
    layout.grid.iter().map(|cell| match cell {
        Cell::Tile { id, .. } => id.to_string(),
        Cell::Shortcut(i) => format!("#{i}"),
        Cell::Add => "+".into(),
    }).collect()
}

#[test]
fn the_grid_keeps_the_model_order_whatever_order_the_client_offers_in() {
    let offers = [Offer::action("quit_game"), Offer::action("reconnect"), Offer::action("shortcuts"),
        Offer::switch("overlay", true), Offer::action("keyboard"), Offer::action("disconnect"), Offer::switch("controller_mouse", false)];
    let layout = layout(&offers, Some(2)).unwrap();
    assert_eq!(ids(&layout), ["keyboard", "controller_mouse", "overlay", "shortcuts", "more", "#0", "#1", "+"]);
    assert_eq!(layout.more, ["reconnect"]);
    assert_eq!(layout.footer, ["close", "disconnect", "quit_game"]);
    assert_eq!(layout.grid[2], Cell::Tile { id: "overlay", on: Some(true), hotkey: Some(Hotkey::Rb) });
}

/// The bottom row has two ways out, Disconnect and Quit game; End session, which leaves the game
/// running on the PC, is the last row under More.
#[test]
fn the_footer_offers_disconnect_and_quit_game_and_end_session_sits_under_more() {
    let offers = [Offer::action("end_host_session"), Offer::action("close_game"), Offer::action("disconnect"), Offer::action("reconnect")];
    let layout = layout(&offers, None).unwrap();
    assert_eq!(layout.footer, ["close", "disconnect", "close_game"]);
    assert_eq!(layout.more, ["reconnect", "end_host_session"]);
    assert!(!assignable(tile("end_host_session").unwrap()), "a button runs what the first page shows");
    let only_end = layout_with_buttons(&[Offer::action("end_host_session")], None, &Buttons::from([(Hotkey::Y, "end_host_session".to_owned())])).unwrap();
    assert_eq!(only_end.footer, ["close"]);
    assert_eq!(only_end.buttons.get(&Hotkey::Y), None, "an old choice of End session leaves Y on its default, which is not offered");
}

#[test]
fn a_button_runs_the_tile_the_user_chose_for_it() {
    let offers = [Offer::action("keyboard"), Offer::switch("controller_mouse", false), Offer::switch("overlay", false),
        Offer::action("shortcuts"), Offer::action("bitrate"), Offer::action("disconnect"), Offer::action("reconnect")];
    let buttons = Buttons::from([
        (Hotkey::Y, "bitrate".to_string()),
        (Hotkey::X, NO_TILE.to_string()),
        (Hotkey::Lb, "disconnect".to_string()),
    ]);
    let menu = layout_with_buttons(&offers, Some(0), &buttons).unwrap();
    // The tile shows its button; a way out under the grid has none to show, and still runs.
    assert_eq!(menu.hotkey(Hotkey::Y).and_then(|i| menu.tile_id(i)), Some("bitrate"));
    assert_eq!(menu.grid[4], Cell::Tile { id: "bitrate", on: None, hotkey: Some(Hotkey::Y) });
    assert_eq!(menu.grid[0], Cell::Tile { id: "keyboard", on: None, hotkey: None });
    assert_eq!(menu.grid[1], Cell::Tile { id: "controller_mouse", on: Some(false), hotkey: None });
    assert_eq!(menu.hotkey(Hotkey::X), None);
    assert_eq!(menu.buttons.get(&Hotkey::X), None);
    assert_eq!(menu.hotkey(Hotkey::Lb), None);
    assert_eq!(menu.buttons[&Hotkey::Lb], "disconnect");
    // A button without a choice keeps its tile.
    assert_eq!(menu.hotkey(Hotkey::Rb).and_then(|i| menu.tile_id(i)), Some("overlay"));
}

#[test]
fn a_button_cannot_be_given_what_the_menu_does_not_show_on_its_first_page() {
    let offers = [Offer::action("keyboard"), Offer::action("reconnect")];
    for id in ["reconnect", "more", "close", "rate_change", "from_a_later_version"] {
        let buttons = Buttons::from([(Hotkey::Y, id.to_string())]);
        let menu = layout_with_buttons(&offers, None, &buttons).unwrap();
        assert_eq!(menu.buttons[&Hotkey::Y], "keyboard", "{id}");
    }
    // A tile this stream does not offer leaves the button doing nothing, not something else.
    let buttons = Buttons::from([(Hotkey::Y, "zoom".to_string())]);
    assert_eq!(layout_with_buttons(&offers, None, &buttons).unwrap().buttons.get(&Hotkey::Y), None);
    // Quality is whichever tile this stream has for it.
    let offers = [Offer::action("rate_change")];
    let buttons = Buttons::from([(Hotkey::Rb, "bitrate".to_string())]);
    assert_eq!(layout_with_buttons(&offers, None, &buttons).unwrap().buttons[&Hotkey::Rb], "rate_change");
}

/// The native Close game and the classic Quit both read Quit game, so a button's setting has one
/// Quit game, and it runs whichever the stream offers; a saved close_game still works.
#[test]
fn quit_game_is_one_choice_and_runs_whichever_way_the_stream_offers() {
    let close = tile("close_game").unwrap();
    assert!(!assignable(close), "one Quit game in the setting");
    assert_eq!(saved_choice("close_game"), "quit_game");
    assert_eq!(saved_choice("overlay"), "overlay");
    for (offered, runs) in [("quit_game", "quit_game"), ("close_game", "close_game")] {
        let offers = [Offer::action("keyboard"), Offer::action("disconnect"), Offer::action(offered)];
        for chosen in ["quit_game", "close_game"] {
            let menu = layout_with_buttons(&offers, None, &Buttons::from([(Hotkey::Lb, chosen.to_string())])).unwrap();
            assert_eq!(menu.buttons.get(&Hotkey::Lb), Some(&runs), "{chosen} with {offered} offered");
        }
    }
    let offers = [Offer::action("keyboard")];
    let menu = layout_with_buttons(&offers, None, &Buttons::from([(Hotkey::Lb, "quit_game".to_string())])).unwrap();
    assert_eq!(menu.buttons.get(&Hotkey::Lb), None, "no way to quit offered: the button does nothing");
}

#[test]
fn two_buttons_on_one_tile_show_the_first_and_both_run_it() {
    let offers = [Offer::switch("overlay", false)];
    let menu = layout_with_buttons(&offers, None, &Buttons::from([(Hotkey::Y, "overlay".to_string())])).unwrap();
    assert_eq!(menu.grid[0], Cell::Tile { id: "overlay", on: Some(false), hotkey: Some(Hotkey::Y) });
    assert_eq!(menu.hotkey(Hotkey::Y), Some(0));
    assert_eq!(menu.hotkey(Hotkey::Rb), Some(0));
}

#[test]
fn the_json_call_takes_the_users_buttons() {
    let answer = json::call(r#"{"op":"layout","offers":[{"id":"zoom"},{"id":"keyboard"}],"buttons":{"y":"zoom"}}"#);
    let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(answer["ok"]["buttons"]["y"], "zoom");
    assert_eq!(answer["ok"]["grid"][1]["tile"]["hotkey"], "y");
}

#[test]
fn hotkeys_reach_only_offered_tiles() {
    let layout = layout(&[Offer::switch("overlay", false), Offer::action("shortcuts")], Some(0)).unwrap();
    assert_eq!(layout.hotkey(Hotkey::Rb), Some(0));
    assert_eq!(layout.hotkey(Hotkey::Lb), Some(1));
    assert_eq!(layout.hotkey(Hotkey::Y), None);
    assert_eq!(layout.hotkey(Hotkey::X), None);
    // Each button belongs to one tile.
    for key in Hotkey::ALL {
        assert_eq!(TILES.iter().filter(|t| t.hotkey == Some(key)).count(), 1, "{key:?}");
    }
    assert!(TILES.iter().filter(|t| t.hotkey.is_some()).all(|t| t.place == Place::Grid));
}

#[test]
fn no_more_tile_without_anything_behind_it_and_no_shortcuts_without_a_keyboard() {
    let layout = layout(&[Offer::action("overlay_page"), Offer::action("disconnect")], None).unwrap();
    assert_eq!(ids(&layout), ["overlay_page"]);
    assert!(layout.more.is_empty());
    let full = super::layout(&[], Some(shortcuts::MAX_SHORTCUTS)).unwrap();
    assert_eq!(full.grid.len(), shortcuts::MAX_SHORTCUTS);
    assert!(!full.grid.contains(&Cell::Add));
    assert_eq!(super::layout(&[], Some(0)).unwrap().grid, [Cell::Add]);
}

#[test]
fn unknown_or_reserved_offers_are_refused() {
    assert_eq!(layout(&[Offer::action("teleport")], None), Err(UnknownTile("teleport".into())));
    assert_eq!(layout(&[Offer::action(MORE)], None), Err(UnknownTile(MORE.into())));
    assert_eq!(layout(&[Offer::action(CLOSE)], None), Err(UnknownTile(CLOSE.into())));
    let mut seen = std::collections::HashSet::new();
    assert!(TILES.iter().all(|t| seen.insert(t.id)), "a tile listed twice");
}

#[test]
fn json_calls_answer_or_name_the_error() {
    let answer: serde_json::Value = serde_json::from_str(&json::call(
        r#"{"op":"layout","offers":[{"id":"overlay","on":true},{"id":"reconnect"}],"shortcuts":1}"#)).unwrap();
    assert_eq!(answer["ok"]["grid"][0], serde_json::json!({"tile": {"id": "overlay", "on": true, "hotkey": "rb"}}));
    assert_eq!(answer["ok"]["grid"][1], serde_json::json!({"tile": {"id": "more"}}));
    assert_eq!(answer["ok"]["grid"][2], serde_json::json!({"shortcut": 0}));
    assert_eq!(answer["ok"]["grid"][3], serde_json::json!("add"));
    assert_eq!(answer["ok"]["more"], serde_json::json!(["reconnect"]));
    assert_eq!(answer["ok"]["footer"], serde_json::json!(["close"]));

    let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"layout","offers":[{"id":"warp"}]}"#)).unwrap();
    assert_eq!(answer["error"], "invalid");
    let answer: serde_json::Value = serde_json::from_str(&json::call("not json")).unwrap();
    assert_eq!(answer["error"], "invalid");

    let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"validate","name":"Snip","keys":["win","Shift","s"]}"#)).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(["WIN", "SHIFT", "S"]));
    let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"validate","name":"Twice","keys":["CTRL","CTRL_LEFT"]}"#)).unwrap();
    assert_eq!((answer["error"].as_str(), answer["detail"].as_str()), (Some("repeated_key"), Some("CTRL_LEFT")));
    let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"validate","name":" ","keys":["A"]}"#)).unwrap();
    assert_eq!(answer["error"], "bad_name");

    let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"built_ins","saved":[["ALT","TAB"],["NOPE"]]}"#)).unwrap();
    let built: Vec<_> = answer["ok"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(built, ["task_manager", "close_window", "start_menu", "show_desktop", "game_bar", "record_that"]);
    assert_eq!(answer["ok"][0]["label"], "Ctrl + Shift + Esc");

    let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"picker"}"#)).unwrap();
    assert_eq!(answer["ok"].as_array().unwrap().len(), shortcuts::PICKER.len());
    assert_eq!(answer["ok"][0], serde_json::json!({"key": "CTRL", "label": "Ctrl", "modifier": true}));
    let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"chord_label","keys":["ALT","F4"]}"#)).unwrap();
    assert_eq!(answer["ok"], "Alt + F4");

    let answer: serde_json::Value =
        serde_json::from_str(&json::call(r#"{"op":"mode_line","width":2560,"height":1440,"fps":144,"format":512,"hdr":true}"#)).unwrap();
    assert_eq!(answer["ok"], "1440p · 144 FPS · HEVC HDR");
    let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"mode_line","width":0,"height":0,"fps":0,"format":0,"hdr":false}"#)).unwrap();
    assert_eq!(answer, serde_json::json!({"ok": null}));
}

/// nightjar#63: a tile has one short name on both clients, so every tile the grid or its footer
/// can show has a label, one key always means the same words, and the words stay short.
#[test]
fn every_tile_in_the_grid_or_under_it_has_one_short_label() {
    for tile in TILES {
        match tile.place {
            Place::Grid => assert!(tile.label.is_some(), "{} has no label", tile.id),
            Place::More => assert_eq!(tile.label, None, "{}: a More row keeps the client's words", tile.id),
            Place::Footer => {}
        }
    }
    for id in ["disconnect", "close_game", "quit_game", MORE, CLOSE, ADD] {
        assert!(label(id).is_some(), "{id}");
    }
    // The footer holds Disconnect and Quit game only; End session is a More row.
    assert_eq!(tile("end_host_session").map(|t| t.place), Some(Place::More));
    assert_eq!(label("end_host_session"), None);
    assert_eq!(label("bitrate"), label("rate_change"), "both change the quality");
    // The native Close game and the classic Quit end the same game: one word for both.
    assert_eq!(label("close_game").map(|l| l.en), label("quit_game").map(|l| l.en));
    assert_eq!(label("controller_mouse").map(|l| l.en), Some("Mouse"));
    assert_eq!(label("server_commands").map(|l| l.en), Some("PC commands"));
    assert_eq!(label("clipboard_receive").map(|l| l.en), Some("Get clipboard"));
    assert_eq!(label("reconnect"), None);
    assert_eq!(label("teleport"), None);
    let all = labels();
    for label in &all {
        let same: Vec<_> = TILES.iter().filter_map(|t| t.label).filter(|l| l.key == label.key).collect();
        assert!(same.iter().all(|l| l == label), "{} names two texts", label.key);
        assert!(label.key.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{} is not a resource name", label.key);
        assert!(label.en.chars().count() <= 20, "{:?} is too long for a tile", label.en);
        assert!(label.en.split(' ').count() <= 3, "{:?} takes more than two lines", label.en);
    }
    assert_eq!(all.len(), all.iter().map(|l| l.key).collect::<std::collections::HashSet<_>>().len());
}

#[test]
fn the_labels_call_names_each_tile_s_key() {
    let answer: serde_json::Value = serde_json::from_str(&json::call(r#"{"op":"labels"}"#)).unwrap();
    assert_eq!(answer["ok"]["bitrate"], "quality");
    assert_eq!(answer["ok"]["rate_change"], "quality");
    assert_eq!(answer["ok"]["close"], "return");
    assert_eq!(answer["ok"]["add"], "add_shortcut");
    assert_eq!(answer["ok"]["more"], "more");
    assert_eq!(answer["ok"]["quit_game"], "quit_game");
    assert!(answer["ok"].get("reconnect").is_none());
    assert!(answer["ok"].get("end_session").is_none());
}

/// nightjar#63: the header says the mode the way players do, "1440p · 144 FPS", with the codec
/// and HDR after it only while the line stays short.
#[test]
fn the_header_line_names_the_mode_like_players_do() {
    assert_eq!(mode_line(2560, 1440, 144, 0, false).as_deref(), Some("1440p · 144 FPS"));
    assert_eq!(mode_line(2560, 1440, 144, 0x0100, false).as_deref(), Some("1440p · 144 FPS · HEVC"));
    assert_eq!(mode_line(1920, 1080, 60, 0x0001, false).as_deref(), Some("1080p · 60 FPS · H.264"));
    assert_eq!(mode_line(3840, 2160, 120, 0x2000, true).as_deref(), Some("4K · 120 FPS · AV1 HDR"));
    assert_eq!(mode_line(2560, 1600, 120, 0x1000, false).as_deref(), Some("1600p · 120 FPS · AV1"), "16:10 is named by its height");
    assert_eq!(mode_line(1080, 1920, 60, 0x0100, false).as_deref(), Some("1080p · 60 FPS · HEVC"), "portrait by its short side");
    assert_eq!(mode_line(1366, 768, 60, 0, false).as_deref(), Some("768p · 60 FPS"));
    assert_eq!(mode_line(1280, 720, 60, 0x01_0000, false).as_deref(), Some("720p · 60 FPS · PyroWave"));
    // Wider than 16:9: the full size, and the codec gives way before HDR when the line runs long.
    assert_eq!(mode_line(3440, 1440, 144, 0x1000, false).as_deref(), Some("3440×1440 · 144 FPS · AV1"));
    assert_eq!(mode_line(3440, 1440, 100, 0x0200, true).as_deref(), Some("3440×1440 · 100 FPS · HDR"));
    assert_eq!(mode_line(5120, 1440, 240, 0x01_0000, false).as_deref(), Some("5120×1440 · 240 FPS"));
    for (w, h, fps) in [(0, 1080, 60), (1920, 0, 60), (1920, 1080, 0)] {
        assert_eq!(mode_line(w, h, fps, 0x0100, true), None, "{w}x{h}@{fps} is not known yet");
    }
    for line in [mode_line(3440, 1440, 100, 0x0200, true), mode_line(3840, 2160, 120, 0x2000, true)] {
        assert!(line.unwrap().chars().count() <= MODE_LINE_MAX);
    }
}
