use super::*;

fn layout(a: &Available, saved: usize) -> Layout {
    super::layout(a, saved, &model::Buttons::new())
}

#[test]
fn a_button_runs_a_way_out_or_a_tile_the_user_gave_it() {
    let buttons = model::Buttons::from([
        (Hotkey::Y, "close_game".to_owned()),
        (Hotkey::X, "disconnect".to_owned()),
        (Hotkey::Lb, "microphone".to_owned()),
        (Hotkey::Rb, model::NO_TILE.to_owned()),
    ]);
    let menu = super::layout(&everything(), 0, &buttons);
    assert_eq!(button_leave(&menu, Hotkey::Y), Some(Leave::Exit(HostExit::CloseGame)));
    assert_eq!(button_leave(&menu, Hotkey::X), Some(Leave::Disconnect));
    assert_eq!(button_leave(&menu, Hotkey::Lb), None);
    assert_eq!(menu.hotkey(Hotkey::Lb).and_then(|index| menu.tile_id(index)), Some("microphone"));
    assert_eq!(button_leave(&menu, Hotkey::Rb), None);
    assert_eq!(menu.hotkey(Hotkey::Rb), None);
    // The ways out show the button that runs them; one without a button shows none.
    assert_eq!(button_cap(&menu, Leave::Exit(HostExit::CloseGame)), Some("Y"));
    assert_eq!(button_cap(&menu, Leave::Disconnect), Some("X"));
    assert_eq!(button_cap(&menu, Leave::Exit(HostExit::QuitApp)), None);
    assert_eq!(button_cap(&super::layout(&everything(), 0, &model::Buttons::new()), Leave::Disconnect), None);
    assert_eq!(Page::ConfirmDisconnect.title(), tr("Disconnect"));
}

#[test]
fn every_choice_for_a_button_is_one_the_desktop_can_draw_and_run() {
    let a = everything();
    for (id, name) in ASSIGNABLE {
        assert!(!tr(name).is_empty());
        if leave(id).is_none() {
            assert_eq!(label(id), tr(name), "{id}");
        }
        let tile = model::TILES.iter().find(|tile| tile.id == id).unwrap();
        assert!(model::assignable(tile), "{id}");
    }
    // Nothing the desktop offers on the first page is missing from the list.
    let menu = layout(&a, 0);
    for cell in &menu.grid {
        if let Cell::Tile { id, .. } = cell {
            assert!(*id == model::MORE || ASSIGNABLE.iter().any(|(tile, _)| tile == id), "{id}");
        }
    }
}

fn everything() -> Available {
    Available {
        mouse: Some(true), overlay: true, shortcuts: true, microphone: Some(false), quality: true, clipboard: true, image_clipboard: true,
        commands: true, session: true, host_audio: true, controller_audio: true, usb: true, guests: true, exits: vec![HostExit::End, HostExit::CloseGame],
    }
}

/// A shortcut file from Android can hold keys the caps lack (F13, the right Ctrl): they get caps of their own,
/// and taking one out leaves the rest to save.
#[test]
fn keys_the_picker_lacks_get_caps_of_their_own_and_can_be_taken_out() {
    let file = r#"{"version":1,"data":[{"id":"a","name":"Old","keys":["CTRL_RIGHT","F13","A"]}]}"#;
    let mut saved = Shortcuts::parse(file).unwrap();
    let mut draft = Draft::edit(&saved.data[0]);
    assert_eq!(extra_keys(&draft.keys), ["CTRL_RIGHT", "F13"]);
    draft.toggle("F13");
    assert_eq!(extra_keys(&draft.keys), ["CTRL_RIGHT"]);
    assert!(draft.save(&mut saved));
    assert_eq!(saved.data[0].keys, ["CTRL_RIGHT", "A"]);
}

/// A shortcut named after its keys says them once: the menu showed "Caps Lock / Caps Lock".
#[test]
fn a_shortcut_named_after_its_keys_shows_no_key_line() {
    assert_eq!(keys_under("Caps Lock", &["CAPS_LOCK".into()]), None);
    assert_eq!(keys_under("Snip", &["WIN".into(), "SHIFT".into(), "S".into()]).as_deref(), Some("Win + Shift + S"));
}

#[test]
fn every_desktop_offer_is_a_tile_the_model_knows_with_a_label() {
    let layout = layout(&everything(), 1);
    let ids: Vec<_> = layout.grid.iter().filter_map(|cell| match cell { Cell::Tile { id, .. } => Some(*id), _ => None }).collect();
    assert_eq!(ids, ["controller_mouse", "overlay", "shortcuts", "microphone", "bitrate", "clipboard_send", "clipboard_receive",
        "server_commands", "native_page", "more"]);
    for id in ids {
        assert!(!label(id).is_empty());
    }
    assert_eq!(layout.grid[layout.grid.len() - 2..], [Cell::Shortcut(0), Cell::Add]);
    assert_eq!(layout.footer, ["close", "disconnect", "close_game"]);
    assert_eq!(layout.more, ["picture", "night_mode", "image_clipboard_send", "image_clipboard_receive", "usb_devices",
        "guests", "controller_audio", "export_statistics", "help", "reconnect", "end_host_session"]);
    for built_in in format::BUILT_INS {
        assert!(!built_in_label(built_in.id).is_empty());
    }
}

/// Android's `stream_tile_<key>` strings in one language's folder (`values`, `values-de`).
fn android_tile_labels(folder: &str) -> std::collections::HashMap<String, String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/main/res").join(folder).join("strings_stream_menu.xml");
    let xml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    xml.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("<string name=\"stream_tile_")?;
            let (key, rest) = rest.split_once("\">")?;
            let text = rest.strip_suffix("</string>")?.replace("\\'", "'").replace("&amp;", "&");
            Some((key.to_owned(), text))
        })
        .collect()
}

/// nightjar#63: a tile has the same short name on the desktop and on Android, in English and in
/// German: the model's label, and Android's string for its key.
#[test]
fn the_desktop_names_each_tile_as_the_model_and_android_do() {
    use crate::strings::{en, tests::with_lang, Lang};
    let layout = layout(&everything(), 0);
    let mut named: Vec<(&str, &str)> = layout.grid.iter().filter_map(|cell| match cell { Cell::Tile { id, .. } => Some(*id), _ => None })
        .map(|id| (id, with_lang(Lang::En, || label(id)))).collect();
    named.push((model::ADD, en("Add shortcut")));
    named.push((model::CLOSE, en("Return to stream")));
    named.push(("disconnect", en("Disconnect")));
    named.extend([HostExit::CloseGame, HostExit::QuitApp].map(|exit| (exit_id(exit), exit.label())));
    let (english, german) = (android_tile_labels("values"), android_tile_labels("values-de"));
    for (id, text) in named {
        let model = model::label(id).unwrap_or_else(|| panic!("{id} has no label in the model"));
        assert_eq!(text, model.en, "{id}: the desktop's English differs from the model's");
        assert_eq!(english.get(model.key).map(String::as_str), Some(model.en), "{id}: Android's stream_tile_{}", model.key);
        let desktop = with_lang(Lang::De, || tr(text).to_owned());
        assert_eq!(german.get(model.key), Some(&desktop), "{id}: Android's German stream_tile_{}", model.key);
    }
    for label in model::labels() {
        assert_eq!(english.get(label.key).map(String::as_str), Some(label.en), "Android has no stream_tile_{}", label.key);
    }
    // The native Close game and the classic Quit end the same game: one word for both, in every
    // language on both clients.
    assert_eq!(HostExit::CloseGame.label(), HostExit::QuitApp.label());
    let res = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/main/res");
    let mut locales = 0;
    for dir in std::fs::read_dir(&res).unwrap().flatten() {
        let folder = dir.file_name().into_string().unwrap();
        if !dir.path().join("strings_stream_menu.xml").exists() {
            continue;
        }
        let android = android_tile_labels(&folder);
        assert_eq!(android.get("close_game"), android.get("quit_game"), "{folder}: Android's two ways to quit the game");
        // One wording per language for quitting a game: the menu's tiles, its confirmation, the game
        // page and the app list's Quit, on Android and in the desktop's table.
        let mut xml = std::fs::read_to_string(dir.path().join("strings_stream_menu.xml")).unwrap();
        xml += &std::fs::read_to_string(dir.path().join("strings.xml")).unwrap_or_default();
        let text = |name: &str| xml.lines().find_map(|line| line.trim().strip_prefix(&format!("<string name=\"{name}\">"))?.strip_suffix("</string>").map(str::to_owned));
        let code = folder.strip_prefix("values-").map(|code| code.replace("-r", "-")).unwrap_or_else(|| "en".to_owned());
        let lang = crate::strings::from_code(&code).unwrap_or_else(|| panic!("{folder} is no desktop language"));
        let desktop = with_lang(lang, || tr("Quit game").to_owned());
        if folder == "values" {
            assert_eq!(desktop, "Quit game");
        }
        assert_eq!(android.get("quit_game"), text("stream_quit_game").as_ref(), "{folder}: the tile and the confirmation word it alike");
        assert_eq!(android.get("quit_game"), Some(&desktop), "{folder}: Android and the desktop word it alike");
        for name in ["stream_tile_close_game", "stream_tile_quit_game", "stream_close_game", "stream_quit_game", "applist_menu_quit"] {
            assert_eq!(text(name).as_deref(), Some(desktop.as_str()), "{folder}: {name} words Quit game otherwise");
        }
        locales += 1;
    }
    assert!(locales >= 14, "{locales} locales");
    for lang in Lang::all() {
        assert_eq!(with_lang(lang, || tr(HostExit::CloseGame.label())), with_lang(lang, || tr(HostExit::QuitApp.label())), "{lang:?}");
    }
}

/// nightjar#63: every way out the model may put in the footer runs on the desktop (no footer ID
/// it cannot draw), Disconnect first and Quit game after it. End session, which leaves the game
/// running, is More's last row instead of a third way out beside them.
#[test]
fn every_way_out_the_desktop_offers_runs_from_the_footer() {
    for exits in [vec![], vec![HostExit::QuitApp], vec![HostExit::End, HostExit::QuitApp], vec![HostExit::End, HostExit::CloseGame]] {
        let layout = layout(&Available { exits: exits.clone(), ..everything() }, 0);
        let leaves: Vec<_> = layout.footer.iter().map(|id| leave(id).unwrap_or_else(|| panic!("{id} has no way to run"))).collect();
        let want: Vec<_> = [Leave::Close, Leave::Disconnect].into_iter().chain(exits.iter().copied().filter(|e| *e != HostExit::End).map(Leave::Exit)).collect();
        assert_eq!(leaves, want);
        assert_eq!(layout.more.last() == Some(&"end_host_session"), exits.contains(&HostExit::End), "{exits:?}");
    }
    assert_eq!(leave("end_session"), None, "a remote monitor's End session is Android's");
    for exit in [HostExit::CloseGame, HostExit::QuitApp] {
        assert_eq!(model::tile(exit_id(exit)).map(|t| t.place), Some(model::Place::Footer));
    }
    assert_eq!(model::tile(exit_id(HostExit::End)).map(|t| t.place), Some(model::Place::More));
}

/// Cancel on End session's question goes back to More, where it was asked; the footer's ways
/// out go back to the first page.
#[test]
fn cancel_goes_back_to_where_the_question_was_asked() {
    assert_eq!(back_from_confirm(HostExit::End), Page::More);
    assert_eq!(back_from_confirm(HostExit::CloseGame), Page::Grid);
    assert_eq!(back_from_confirm(HostExit::QuitApp), Page::Grid);
}

/// One frame in the menu's window: each named widget's place, and the focused one's name.
fn frame(ctx: &egui::Context, add: impl FnOnce(&mut Ui)) -> (Vec<(String, egui::Rect)>, Option<String>) {
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
    let mut add = Some(add);
    let mut output = ctx.run_ui(input, |ui| {
        menu_style::show(ui.ctx(), "Stream menu", "footer-test", |ui| (add.take().unwrap())(ui));
    });
    output.textures_delta.clear();
    let tree = output.platform_output.accesskit_update.expect("AccessKit is on");
    let focus = tree.nodes.iter().find(|(id, _)| *id == tree.focus).and_then(|(_, n)| n.label()).map(str::to_owned);
    let nodes = tree.nodes.iter().filter_map(|(_, n)| {
        let b = n.bounds()?;
        Some((n.label()?.to_owned(), egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))))
    }).collect();
    (nodes, focus)
}

/// The shot over a bright frame showed a hard line above the focused tile: the launcher's glow
/// reaches further than a menu page's clip. Tabbing through the whole panel, every glow ends
/// inside what clips it and inside the panel, and a glow outside a menu keeps its reach.
#[test]
fn a_focused_controls_glow_ends_before_anything_cuts_it() {
    let ctx = egui::Context::default();
    crate::theme::own_palette();
    crate::theme::apply(&ctx);
    let layout = layout(&everything(), 0);
    let saved = Shortcuts::default();
    ctx.enable_accesskit();
    let mut buttons = std::collections::BTreeSet::new();
    let mut focused = std::collections::BTreeSet::new();
    // Every control in the order Tab reached them.
    let mut order: Vec<(egui::accesskit::NodeId, String)> = Vec::new();
    // The panel fades in first, and a glow carries its opacity. A Tab, then a pass with no key,
    // as a real press comes.
    for pass in 0..120 {
        let events = if pass < 30 || pass % 2 == 1 {
            Vec::new()
        } else {
            vec![egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]
        };
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 800.0))), events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            menu_style::show(ui.ctx(), "Stream menu", "glow-test", |ui| {
                menu_style::head(ui, "Racing Game", "1080p");
                let reserve = menu_style::pinned_room(ui.ctx(), "glow-footer");
                // As the real menu asks it.
                let focus_first = ui.ctx().memory(|m| m.focused().is_none());
                menu_style::body_above(ui, reserve, |ui| grid(ui, &layout, &saved, None, focus_first));
                menu_style::pinned(ui, "glow-footer", |ui| footer(ui, &layout));
            });
        });
        output.textures_delta.clear();
        let tree = output.platform_output.accesskit_update.clone().expect("the tree");
        buttons.extend(tree.nodes.iter().filter(|(_, n)| n.role() == egui::accesskit::Role::Button).filter_map(|(_, n)| n.label().map(str::to_owned)));
        let on = tree.nodes.iter().find(|(id, _)| *id == tree.focus).and_then(|(_, n)| n.label()).map(str::to_owned);
        let mut glows = 0;
        let panel = ctx.memory(|m| m.area_rect(egui::Id::unique("glow-test"))).expect("the panel's place");
        for shape in &output.shapes {
            let egui::Shape::Mesh(mesh) = &shape.shape else { continue };
            if mesh.vertices.first().is_none_or(|v| v.color != crate::ui::design_tokens::FOCUS_HALO_COLOR) {
                continue;
            }
            let glow = mesh.calc_bounds();
            glows += 1;
            assert!(shape.clip_rect.expand(0.6).contains_rect(glow), "{glow:?} is cut by its clip {:?}", shape.clip_rect);
            assert!(panel.expand(0.6).contains_rect(glow), "{glow:?} leaves the panel {panel:?}");
        }
        if pass >= 30 {
            if let Some(on) = on {
                assert_eq!(glows, 1, "{on} glows");
                if order.last().is_none_or(|(id, _)| *id != tree.focus) {
                    order.push((tree.focus, on.clone()));
                }
                focused.insert(on);
            }
        }
    }
    // The loop: the last way out wraps to the head's close, and the close to the first tile.
    let said: Vec<&str> = order.iter().map(|(_, on)| on.as_str()).collect();
    let last = said.iter().position(|on| on.starts_with("Quit game")).expect("Quit game");
    assert_eq!(said.get(last + 1), Some(&"Return to stream"), "after the last way out: {said:?}");
    assert!(said.get(last + 2).is_some_and(|on| on.starts_with("Mouse")), "{said:?}");
    for way in ["Return to stream", "Disconnect", "Quit game"] {
        assert!(buttons.iter().any(|b| b.starts_with(way)), "{way} in {buttons:?}");
    }
    assert!(buttons.len() > 8, "the tiles too: {buttons:?}");
    let missed: Vec<_> = buttons.difference(&focused).collect();
    assert!(missed.is_empty(), "the tiles, the close and the ways out each take the focus by Tab; not {missed:?}");
    let mut out = Vec::new();
    crate::ui::glass::focus(&mut out, &ctx, egui::Rect::from_min_size(egui::pos2(100.0, 100.0), vec2(80.0, 40.0)), 12.0, false);
    let egui::Shape::Mesh(mesh) = &out[0] else { panic!("the glow comes first") };
    let reach = 100.0 - crate::ui::design_tokens::FOCUS_GAP - crate::ui::design_tokens::FOCUS_RING_WIDTH - mesh.calc_bounds().left();
    assert!((reach - crate::ui::design_tokens::FOCUS_HALO_BLUR).abs() < 1.0, "outside a menu the glow keeps its reach: {reach}");
}

/// Disconnect and Quit game side by side in equal widths, as Starwire's menu has them; End
/// session is not a third one.
#[test]
fn the_ways_out_sit_side_by_side_across_the_panel() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    ctx.enable_accesskit();
    let layout = layout(&everything(), 0);
    let mut nodes = Vec::new();
    for _ in 0..4 {
        nodes = frame(&ctx, |ui| {
            footer(ui, &layout);
        })
        .0;
    }
    let find = |name: &str| nodes.iter().find(|(label, _)| label.starts_with(name)).map(|(_, r)| *r).unwrap_or_else(|| panic!("{name} in {nodes:#?}"));
    let (disconnect, close) = (find("Disconnect"), find("Quit game"));
    assert!((close.top() - disconnect.top()).abs() < 1.0, "one row: {close:?} beside {disconnect:?}");
    assert!((close.width() - disconnect.width()).abs() < 1.5, "equal widths");
    assert!(disconnect.right() < close.left(), "Disconnect, then Quit game");
    assert!(!nodes.iter().any(|(label, _)| label.starts_with("End session")), "End session is under More");
    assert!(!nodes.iter().any(|(label, _)| label.starts_with("Return to stream")), "the head's round close does that");
    let spoken = nodes.iter().find(|(label, _)| label.starts_with("Disconnect")).unwrap().0.clone();
    assert!(spoken.starts_with("Disconnect, "), "Disconnect's keys are read out: {spoken}");
}

/// nightjar#61: Game Bar and Record that, new built-ins, landed ahead of a user's own list. The
/// saved shortcuts keep their order at the start, whatever built-ins a version adds.
#[test]
fn the_saved_shortcuts_come_before_the_built_ins() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    ctx.enable_accesskit();
    let mut saved = Shortcuts::default();
    saved.data.push(Shortcut { id: "s".into(), name: "Steam overlay".into(), keys: vec!["SHIFT".into(), "TAB".into()] });
    let mut nodes = Vec::new();
    for _ in 0..4 {
        nodes = frame(&ctx, |ui| {
            assert_eq!(shortcuts_page(ui, &saved, false), None);
        })
        .0;
    }
    // A saved shortcut also has a row to edit it, under the tiles: the topmost is the tile.
    let find = |name: &str| {
        nodes.iter().filter(|(label, _)| label.starts_with(name)).map(|(_, r)| *r).min_by(|a, b| a.top().total_cmp(&b.top())).unwrap_or_else(|| panic!("{name} in {nodes:#?}"))
    };
    let (own, task_manager, add) = (find("Steam overlay"), find(built_in_label("task_manager")), find(tr("Add shortcut")));
    let reading_order = |a: egui::Rect, b: egui::Rect| a.top() < b.top() - 1.0 || ((a.top() - b.top()).abs() <= 1.0 && a.left() < b.left());
    assert!(reading_order(own, task_manager), "{own:?} before {task_manager:?}");
    assert!(reading_order(task_manager, add), "{task_manager:?} before {add:?}");
}

/// Ending the PC's game asks first, with the focus on Cancel so A pressed twice never ends it.
#[test]
fn a_way_out_asks_first_with_cancel_focused() {
    crate::theme::own_palette();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    ctx.enable_accesskit();
    let (mut nodes, mut focus) = (Vec::new(), None);
    for n in 0..3 {
        let (all, focused) = frame(&ctx, |ui| {
            assert_eq!(confirm(ui, HostExit::CloseGame, n == 0), None);
        });
        nodes.extend(all);
        focus = focused.or(focus);
    }
    assert!(nodes.iter().any(|(label, _)| label == tr("Quit game")), "{nodes:#?}");
    assert_eq!(focus.as_deref(), Some(tr("Cancel")));
    assert_eq!(Page::Confirm(HostExit::CloseGame).title(), tr("Quit game"));
}

#[test]
fn a_remote_monitor_gets_no_shortcuts_and_the_buttons_reach_their_tiles() {
    let layout = layout(&Available { shortcuts: false, ..everything() }, 3);
    assert!(!layout.grid.iter().any(|cell| matches!(cell, Cell::Shortcut(_) | Cell::Add)));
    assert_eq!(layout.hotkey(Hotkey::Lb), None);
    let layout = self::layout(&everything(), 0);
    assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_WEST).and_then(|k| layout.hotkey(k)).and_then(|i| layout.tile_id(i)), Some("controller_mouse"));
    assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER).and_then(|k| layout.hotkey(k)).and_then(|i| layout.tile_id(i)), Some("overlay"));
    assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_LEFT_SHOULDER).and_then(|k| layout.hotkey(k)).and_then(|i| layout.tile_id(i)), Some("shortcuts"));
    assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_NORTH).and_then(|k| layout.hotkey(k)), None, "no keyboard tile on the desktop");
    assert_eq!(hotkey(SDL_GAMEPAD_BUTTON_SOUTH), None);
}

#[test]
fn a_shortcut_written_in_the_stream_is_saved_named_after_its_keys_and_can_be_deleted() {
    let mut saved = Shortcuts::default();
    let mut draft = Draft::default();
    for key in ["WIN", "SHIFT", "S", "S"] {
        draft.toggle(key);
    }
    assert_eq!(draft.keys, ["WIN", "SHIFT"]);
    draft.toggle("S");
    assert!(draft.save(&mut saved));
    assert_eq!((saved.data[0].name.as_str(), &saved.data[0].keys[..]), ("Win + Shift + S", &["WIN".to_owned(), "SHIFT".into(), "S".into()][..]));
    let mut edit = Draft::edit(&saved.data[0]);
    edit.name = "Snip".into();
    assert!(edit.save(&mut saved));
    assert_eq!(saved.data.len(), 1);
    assert_eq!(saved.data[0].name, "Snip");
    let mut empty = Draft::default();
    assert!(!empty.save(&mut saved));
    assert_eq!(empty.error.as_deref(), Some(tr("Use between 1 and 8 keys in each chord.")));
    for key in format::PICKER.iter().take(9) {
        empty.toggle(key);
    }
    assert_eq!(empty.keys.len(), format::MAX_KEYS, "a ninth key is not taken");
}

/// Named in the order it is saved and pressed, not the order picked: a Caps Lock then Shift
/// draft was named "Caps Lock + Shift" though it sends Shift first.
#[test]
fn a_draft_is_named_modifiers_first_whatever_order_its_keys_were_picked_in() {
    let mut saved = Shortcuts::default();
    let mut draft = Draft::default();
    draft.toggle("CAPS_LOCK");
    draft.toggle("SHIFT");
    assert!(draft.save(&mut saved));
    assert_eq!((saved.data[0].name.as_str(), &saved.data[0].keys[..]), ("Shift + Caps Lock", &["SHIFT".to_owned(), "CAPS_LOCK".into()][..]));
}
