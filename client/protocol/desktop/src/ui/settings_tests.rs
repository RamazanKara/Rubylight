//! The settings page as people use it: sections and their folds, search, reset to default and the
//! PC and game layers, driven through whole frames like the launcher's other tests.

use super::*;
use nightjar_policy::presets::{self, Preset};

/// Main settings at their defaults but `changed`, on a tall window where a section never scrolls.
fn settings_with(changed: Settings) -> Frames {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.l.settings = changed;
    f.l.go(Screen::Settings);
    f.settle();
    f
}

/// Opens every section's fold, as its "More … settings" row does.
fn open_folds(f: &mut Frames) {
    f.l.settings_view.more = [true; 8];
    f.settle();
}

/// The open section's fold row as the last frame wrote it, if it has one.
fn fold_words(f: &Frames) -> Option<String> {
    f.drawn.iter().find(|t| (t.starts_with("More ") || t.starts_with("Fewer ")) && t.contains(" settings")).cloned()
}

/// The rows the last frame drew that hold a setting, by their first key (action rows left out).
fn setting_keys(f: &Frames) -> Vec<&'static str> {
    f.l.settings_view.drawn.iter().map(|(key, _)| *key).filter(|key| !ACTION_ROWS.contains(key)).collect()
}

/// The words right of the row labelled `label`: its control's, and nothing of another row.
fn row_words<'f>(f: &'f Frames, label: &str) -> Vec<&'f str> {
    let at = f.placed.iter().find(|(t, ..)| t == label).unwrap_or_else(|| panic!("{label} is not drawn: {:?}", f.drawn)).1;
    f.placed.iter().filter(|(t, r, _)| t != label && r.left() > at.right() && (r.center().y - at.center().y).abs() < 24.0).map(|(t, ..)| t.as_str()).collect()
}

/// Clicks `text` in the row labelled `label`: its value, which opens its list.
fn click_value(f: &mut Frames, label: &str, text: &str) -> Vec<Command> {
    let row = f.placed.iter().find(|(t, ..)| t == label).unwrap_or_else(|| panic!("{label} is not drawn: {:?}", f.drawn)).1;
    let at = f.placed.iter().find(|(t, r, _)| t == text && r.left() > row.right() && (r.center().y - row.center().y).abs() < 24.0)
        .unwrap_or_else(|| panic!("{text:?} is not in {label}: {:?}", row_words(f, label))).1.center();
    f.click_at(at, egui::PointerButton::Primary)
}

/// Clicks the last `text` drawn: an open list's entry rather than a row's value.
fn click_last(f: &mut Frames, text: &str) -> Vec<Command> {
    let at = f.placed.iter().rev().find(|(t, ..)| t == text).unwrap_or_else(|| panic!("{text:?} is not drawn: {:?}", f.drawn)).1.center();
    f.click_at(at, egui::PointerButton::Primary)
}

/// Presses `key` until focus lands on the widget drawn with `text`. False when it never does.
fn focus_to(f: &mut Frames, key: Key, text: &str) -> bool {
    for _ in 0..16 {
        f.press(key);
        let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        let at = f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1.center());
        if focused.zip(at).is_some_and(|(r, at)| r.contains(at)) {
            return true;
        }
    }
    false
}

#[test]
fn the_d_pad_scrolls_each_row_it_reaches_into_view() {
    // The main settings and a PC's own, on a window too short for a section. Video, the first, is
    // short at its defaults (its developer rows sit in Debug, its dependent rows hide), so its fold
    // is open and Automatic quality shows the two rows that follow it.
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 600.0);
    f.l.settings = Settings { adaptive_quality: crate::config::QualityMode::Motion, ..Settings::default() };
    f.l.settings_view.more = [true; 8];
    let id = f.hosts.list[0].id;
    for page in [Screen::Settings, Screen::HostSettings(id)] {
        if page == Screen::Settings { f.l.go(page) } else { f.l.open_host_settings(id, &f.hosts.list[0]) }
        f.settle();
        let first = f.ctx.memory(|m| m.focused()).expect("the first row has focus");
        let top = f.ctx.read_response(first).expect("laid out").rect.top();
        let mut reached = std::collections::HashSet::new();
        for _ in 0..24 {
            f.press(Key::ArrowDown);
            // The scroll animates.
            f.settle();
            f.settle();
            let focused = f.ctx.memory(|m| m.focused()).expect("a row has focus");
            reached.insert(focused);
            let rect = f.ctx.read_response(focused).expect("laid out").rect;
            assert!(rect.top() >= 0.0 && rect.bottom() <= f.size.y, "{page:?}: the focused row {rect:?} is in the window");
        }
        let moved = top - f.ctx.read_response(first).expect("laid out").rect.top();
        assert!(reached.len() > 6 && moved > 100.0, "{page:?}: Down went past the window's edge and the page scrolled ({} rows, {moved})", reached.len());
    }
}

#[test]
fn the_search_finds_rows_in_every_section_and_says_when_nothing_matches() {
    let mut f = settings_with(Settings { microphone: true, ..Settings::default() });
    f.l.settings_view.query = "microphone".into();
    f.settle();
    // Rows behind a closed fold too.
    for text in ["Results for “microphone”", "Audio", "Send microphone to the PC", "Echo cancellation", "Microphone gain", "Controls", "USB DualSense audio and haptics"] {
        assert!(shows(&f, text), "{text} is found: {:?}", f.drawn);
    }
    assert!(!shows(&f, "Resolution") && !shows(&f, "Channels"), "rows without the word hide: {:?}", f.drawn);
    assert_eq!(fold_words(&f), None, "a search draws no fold");
    // A section's name finds its rows; every word must match somewhere.
    f.l.settings_view.query = "audio channels".into();
    f.settle();
    assert!(shows(&f, "Channels") && !shows(&f, "Send microphone to the PC"), "{:?}", f.drawn);
    f.l.settings_view.query = "zzzz".into();
    f.settle();
    assert!(shows(&f, "No settings match “zzzz”") && !shows(&f, "Channels"), "{:?}", f.drawn);
    click_text(&mut f, "Clear search");
    assert!(f.l.settings_view.query.is_empty() && shows(&f, "Resolution"), "Clear search goes back to the section: {:?}", f.drawn);
}

/// The Ctrl+F hint shows only where the empty field's placeholder leaves it room, in every language.
#[test]
fn the_ctrl_f_hint_never_covers_the_search_placeholder() {
    let mut f = settings_with(Settings::default());
    for language in crate::config::Language::ALL.into_iter().filter(|l| *l != crate::config::Language::System) {
        f.l.settings.language = language;
        for width in [640.0, 800.0, 1024.0, 1280.0] {
            f.size = vec2(width, 4000.0);
            f.settle();
            let at = |text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|(_, r, _)| *r);
            let hint = at(tr("Search settings")).unwrap_or_else(|| panic!("{language:?} at {width}: no placeholder in {:?}", f.drawn));
            if let Some(key) = at(tr("Ctrl+F")) {
                assert!(hint.right() + 8.0 <= key.left(), "{language:?} at {width}: the placeholder {hint:?} runs into Ctrl+F {key:?}");
            }
        }
    }
}

#[test]
fn ctrl_f_reaches_the_search_and_escape_leaves_it_before_the_page() {
    let mut f = settings_with(Settings::default());
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    assert_eq!(f.ctx.memory(|m| m.focused()), Some(Id::unique("settings-search")), "Ctrl+F focuses the search");
    f.frame(vec![egui::Event::Text("hdr".into())]);
    f.settle();
    assert_eq!(f.l.settings_view.query, "hdr");
    assert!(shows(&f, "HDR") && !shows(&f, "Resolution"), "{:?}", f.drawn);
    // Escape leaves the field, then clears the search, then leaves the page.
    let commands = f.press(Key::Escape);
    assert!(f.l.screen == Screen::Settings && commands.is_empty() && f.l.settings_view.query == "hdr", "{}", commands.len());
    let commands = f.press(Key::Escape);
    assert!(f.l.screen == Screen::Settings && commands.is_empty() && f.l.settings_view.query.is_empty(), "{}", commands.len());
    let commands = f.press(Key::Escape);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "then Escape leaves");
}

#[test]
fn a_section_opens_from_its_entry_by_mouse_and_by_controller() {
    let mut f = settings_with(Settings::default());
    assert!(shows(&f, "Resolution") && !shows(&f, "Channels"));
    click_text(&mut f, "Audio");
    assert!(shows(&f, "Channels") && !shows(&f, "Resolution"), "{:?}", f.drawn);
    // The D-pad: from the rows left to the list, down to Streaming, A opens it.
    f.l.go(Screen::Settings);
    f.l.settings_view.section = "Video";
    f.settle();
    f.press(Key::ArrowLeft);
    let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let rows = f.placed.iter().find(|(t, ..)| t == "Resolution").expect("the first row").1;
    assert!(focused.is_some_and(|r| r.right() < rows.left()), "Left reaches the search or the list: {focused:?}");
    assert!(focus_to(&mut f, Key::ArrowDown, "Streaming"), "Down walks the list");
    f.press(Key::Enter);
    assert!(shows(&f, "Recover interrupted streams") && !shows(&f, "Resolution"), "{:?}", f.drawn);
    // Opening a section during a search ends the search.
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    click_text(&mut f, "Audio");
    assert!(f.l.settings_view.query.is_empty() && shows(&f, "Channels"), "{:?}", f.drawn);
}

/// Each section shows its main rows (Android's `settings_tiers.xml`), then one row that says how
/// many more it holds and opens them; it closes them again. A search finds folded rows anyway.
#[test]
fn a_sections_fold_counts_its_rows_and_opens_and_closes() {
    let mut f = settings_with(Settings::default());
    assert_eq!(f.l.settings_view.more, [false; 8]);
    assert!(f.l.settings_view.drawn.iter().all(|(_, folded)| !folded), "only main rows: {:?}", f.l.settings_view.drawn);
    assert!(!shows(&f, "Sharper colored text"), "{:?}", f.drawn);
    let more = fold_words(&f).expect("Video has a fold");
    click_text(&mut f, &more);
    assert!(f.l.settings_view.more[0] && shows(&f, "Fewer Video settings") && shows(&f, "Sharper colored text"), "{:?}", f.drawn);
    let drawn = f.l.settings_view.drawn.clone();
    let folded = drawn.iter().filter(|(_, folded)| *folded).count();
    assert_eq!(more, format!("More Video settings ({folded})"), "the count is the rows it opens: {drawn:?}");
    let first = drawn.iter().position(|(_, folded)| *folded).expect("a folded row");
    assert!(drawn[first..].iter().all(|(_, folded)| *folded), "the main rows come first: {drawn:?}");
    // A fold is the section's own: Audio's stays closed.
    click_text(&mut f, "Audio");
    assert!(shows(&f, "Channels") && !shows(&f, "Mute audio") && fold_words(&f).is_some_and(|w| w.starts_with("More Audio settings")), "{:?}", f.drawn);
    click_text(&mut f, "Video");
    click_text(&mut f, "Fewer Video settings");
    assert!(!f.l.settings_view.more[0] && !shows(&f, "Sharper colored text"), "{:?}", f.drawn);
    // The search finds a folded row, and draws no fold.
    f.l.settings_view.query = "colored text".into();
    f.settle();
    assert!(shows(&f, "Sharper colored text") && fold_words(&f).is_none(), "{:?}", f.drawn);
}

/// A row shows its label and value; its one-line hint comes while the pointer is over it or focus
/// is in it, without moving the rows below.
#[test]
fn a_rows_hint_shows_on_hover_and_focus_only() {
    let mut f = settings_with(Settings::default());
    let (size_hint, rate_hint) = ("The picture size the PC sends. Higher is sharper and needs more network and decoding power; lower it if the stream stutters.",
        "Frames per second (FPS). Higher looks smoother and needs a faster network and more decoding power; lower it if the stream stutters.");
    // Resolution has focus on arrival.
    assert!(shows(&f, size_hint) && !shows(&f, rate_hint), "{:?}", f.drawn);
    let y = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).unwrap_or_else(|| panic!("{text}")).1.top();
    let below = y(&f, "Video codec");
    let rate = f.placed.iter().find(|(t, ..)| t == "Frame rate").expect("the row").1;
    f.frame(vec![egui::Event::PointerMoved(rate.center())]);
    f.settle();
    assert!(shows(&f, rate_hint), "{:?}", f.drawn);
    assert!((y(&f, "Video codec") - below).abs() < 0.5, "the rows below stay");
    // One line: a hint longer than the room is cut, not wrapped.
    f.size = vec2(980.0, 4000.0);
    f.settle();
    let rate = f.placed.iter().find(|(t, ..)| t == "Frame rate").expect("the row").1;
    f.frame(vec![egui::Event::PointerMoved(rate.center())]);
    f.settle();
    let hint = f.placed.iter().find(|(t, ..)| t == rate_hint).expect("the hint").1;
    assert!(hint.height() < 24.0, "{hint:?}");
}

#[test]
fn a_changed_row_offers_its_default_back() {
    let mut f = settings_with(Settings { fps: 120, ..Settings::default() });
    let resets: Vec<_> = f.placed.iter().filter(|(t, ..)| t == "Reset").collect();
    assert_eq!(resets.len(), 1, "only the changed row has a Reset: {:?}", f.drawn);
    click_text(&mut f, "Reset");
    assert_eq!(f.l.settings.fps, Settings::default().fps);
    assert!(!shows(&f, "Reset") && f.l.settings_dirty, "{:?}", f.drawn);
}

#[test]
fn reset_all_asks_first_and_keeps_the_language() {
    let mut f = settings_with(Settings { fps: 120, hdr: true, language: Language::English, ..Settings::default() });
    click_text(&mut f, "App");
    let more = fold_words(&f).expect("App has a fold");
    click_text(&mut f, &more);
    click_text(&mut f, "Reset all…");
    assert!(shows(&f, "Reset all settings?") && shows(&f, "Cancel"), "{:?}", f.drawn);
    // The question says what stays.
    assert!(shows(&f, "Every setting on this page goes back to its default. Your language, PCs, games, profiles, PIN lock, controller buttons, shortcuts and anything not on this page stay as they are. This can’t be undone."), "{:?}", f.drawn);
    click_text(&mut f, "Cancel");
    assert!(f.l.settings.fps == 120 && shows(&f, "Reset all…"), "Cancel changes nothing");
    click_text(&mut f, "Reset all…");
    click_text(&mut f, "Reset all");
    assert_eq!((f.l.settings.fps, f.l.settings.hdr, f.l.settings.language), (0, false, Language::English));
    assert!(f.l.settings_dirty && shows(&f, "Reset all…"), "{:?}", f.drawn);
}

#[test]
fn a_pcs_row_says_whether_it_follows_the_main_settings_or_its_own() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"fps":120}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let main_fps = value_text("fps", &f.l.inherited_settings, &f.l.display, crate::glyphs::Style::Xbox);
    for text in ["1 setting is set here.", "From main settings", "For this PC", &format!("Otherwise {main_fps}")] {
        assert!(shows(&f, text), "{text}: {:?}", f.drawn);
    }
    assert!(!shows(&f, "Reset"), "a PC's rows go back with their box, not Reset");
    // Unticking the Frame rate's box gives the row back to the main settings.
    let menu = f.placed.iter().find(|(t, ..)| t == "120 fps").expect("the PC's own rate").1;
    let boxed = f.placed.iter().filter(|(t, r, _)| t == "For this PC" && r.top() >= menu.bottom()).map(|p| p.1)
        .min_by(|a, b| a.top().total_cmp(&b.top())).expect("its box");
    f.click_at(boxed.center(), egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.is_empty() && shows(&f, "Nothing is set here yet."), "{:?}", f.drawn);
    // A PC's settings have no App section and no Profiles.
    assert!(!shows(&f, "App") && !shows(&f, "Profiles"), "{:?}", f.drawn);
}

/// A search set from code (or typed) after scrolling a long section shows its heading, not the
/// section's old scroll position.
#[test]
fn a_new_search_shows_its_results_from_the_top() {
    let mut f = settings_with(Settings::default());
    f.size = vec2(1280.0, 720.0);
    f.l.settings_view.section = "Controls";
    open_folds(&mut f);
    f.scroll_to_end();
    let seen = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).is_some_and(|(_, r, clip)| r.top() >= clip.top());
    assert!(!seen(&f, "Mouse mode"), "the section scrolled: {:?}", f.drawn);
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    assert!(seen(&f, "Results for “bitrate”"), "the results start at the top: {:?}", f.placed.iter().find(|(t, ..)| t.starts_with("Results")));
}

/// While searching, a section without matches shows no count, and its entry still opens it.
#[test]
fn a_section_without_matches_shows_no_count_and_opens_from_its_entry() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    let found = f.l.settings_view.found;
    assert!(found.contains(&0) && found.iter().any(|&n| n > 0), "{found:?}");
    let rows_left = f.placed.iter().find(|(t, ..)| t == "Results for “bitrate”").expect("the results").1.left();
    let counts: Vec<&str> = f.placed.iter().filter(|(t, r, _)| r.right() < rows_left && t.parse::<usize>().is_ok()).map(|(t, ..)| t.as_str()).collect();
    let expected: Vec<String> = found.iter().filter(|&&n| n > 0).map(usize::to_string).collect();
    assert_eq!(counts, expected, "only the sections with matches show a count");
    click_text(&mut f, "Display & image");
    assert!(f.l.settings_view.query.is_empty() && f.l.settings_view.section == "Display & image", "{:?}", f.l.settings_view.query);
    assert!(shows(&f, "Video size") && !shows(&f, "Automatic bitrate"), "{:?}", f.drawn);
}

/// Profiles sit above the sections, a page of their own as on Android; a PC's settings have none.
#[test]
fn profiles_sit_above_the_sections_and_open_their_page() {
    let mut f = settings_with(Settings::default());
    let top = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1.top());
    assert!(top(&f, "Profiles").zip(top(&f, "Video")).is_some_and(|(p, v)| p < v), "{:?}", f.drawn);
    let commands = click_text(&mut f, "Profiles");
    assert_eq!(f.l.screen, Screen::Profiles);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "{} commands", commands.len());
    // A narrow window has its chip first.
    f.l.go(Screen::Settings);
    f.size = vec2(800.0, 4000.0);
    f.settle();
    let chips: Vec<&str> = f.drawn.iter().map(String::as_str).filter(|t| *t == "Profiles" || SETTINGS_SECTIONS.contains(t)).collect();
    assert_eq!(chips[..2], ["Profiles", "Video"], "{:?}", f.drawn);
}

/// A PC's row box: unticked and ticked as a checkbox for readers, named for its row, and a click
/// keeps the row's value for the PC.
#[test]
fn a_pcs_row_box_is_a_named_checkbox_that_toggles() {
    use egui::accesskit::{Role, Toggled};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let frame_rate_box = |f: &Frames| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("For this PC") && n.description() == Some("Frame rate"))
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
            .expect("the Frame rate row's box")
    };
    let (toggled, bounds) = frame_rate_box(&f);
    assert_eq!(toggled, Some(Toggled::False));
    assert!(bounds.y1 - bounds.y0 >= 24.0, "a box easy to hit: {bounds:?}");
    let at = Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    f.click_at(at, egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.has(&["fps"]), "the click keeps Frame rate for this PC");
    assert_eq!(frame_rate_box(&f).0, Some(Toggled::True));
    f.click_at(at, egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.is_empty() && frame_rate_box(&f).0 == Some(Toggled::False), "a second click gives it back");
}

/// A PC's row box sits on the row's last line, beside where the value comes from, under a short
/// switch as under a menu.
#[test]
fn a_pcs_row_box_lines_up_with_where_the_value_comes_from() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    for row in ["Resolution", "Automatic bitrate", "Lower bitrate when the network is busy"] {
        let top = f.placed.iter().find(|(t, ..)| t == row).expect("the row").1.top();
        let below = |text: &str| f.placed.iter().filter(|(t, r, _)| t == text && r.top() > top).map(|p| p.1)
            .min_by(|a, b| a.top().total_cmp(&b.top())).unwrap_or_else(|| panic!("{text} under {row}"));
        let (state, boxed) = (below("From main settings"), below("For this PC"));
        assert!((state.center().y - boxed.center().y).abs() < 3.0, "{row}: {state:?} beside {boxed:?}");
    }
}

/// Left from a row's control lands on the open section's entry (or chip), not on the widget
/// egui finds nearest to the left: on a wide window that is the search or the top of the list,
/// on a narrow one the chip nearest the rows.
#[test]
fn left_from_the_rows_goes_to_the_open_sections_entry() {
    for (width, section) in [(1280.0, "Advanced"), (1280.0, "Audio"), (900.0, "Video"), (900.0, "Audio")] {
        let mut f = settings_with(Settings::default());
        f.size = vec2(width, 4000.0);
        f.l.go(Screen::Settings);
        f.l.settings_view.section = section;
        f.settle();
        let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        let first = focused(&f).expect("the first row's control has focus");
        f.press(Key::ArrowLeft);
        // The entry or chip, in smaller type than the section's heading over the rows.
        let entry = f.placed.iter().filter(|(t, ..)| t == section).map(|p| p.1)
            .min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
        assert!(entry.right() < first.left() || entry.bottom() < first.top(), "{width}: {entry:?} is the entry");
        assert!(focused(&f).is_some_and(|r| r.contains(entry.center())), "{width} {section}: Left lands on {:?}, not the entry {entry:?}", focused(&f));
        if width < 940.0 {
            continue;
        }
        // Right from the list goes back into the rows.
        f.press(Key::ArrowRight);
        assert!(focused(&f).is_some_and(|r| r.left() > entry.right()), "{width} {section}: Right leaves the entry: {:?}", focused(&f));
    }
}

/// The profile notice's line sits beside its button's middle, not at the button's top.
#[test]
fn the_profile_notice_lines_up_with_its_button() {
    let mut f = settings_with(Settings::default());
    let travel = f.l.profiles.create("Travel", serde_json::from_str(r#"{"bitrate_kbps":20000}"#).unwrap()).unwrap();
    f.l.profiles.active = Some(travel);
    f.settle();
    let at = |text: &str| f.placed.iter().find(|(t, ..)| t.starts_with(text)).unwrap_or_else(|| panic!("{text}: {:?}", f.drawn)).1.center().y;
    let (line, button) = (at("Streams on this network use the profile Travel."), at("Manage profiles"));
    assert!((line - button).abs() < 2.0, "the notice at {line}, its button at {button}");
}

/// SECTION_KEYS lists every setting row in the order drawn with the folds open, so a PC's count
/// and Reset all miss none.
#[test]
fn the_section_key_table_is_what_the_page_draws() {
    // Every setting another one shows on.
    let changed = Settings {
        bitrate_kbps: 30_000,
        adaptive_quality: crate::config::QualityMode::Motion,
        upscaling: crate::config::Upscaling::Fsr1,
        virtual_display: true,
        controller_mouse: true,
        microphone: true,
        overlay: true,
        client_roaming: true,
        dual_screen: true,
        pyrowave_decoder: crate::config::PyroWaveDecoder::Gpu,
        decoder: DecoderPreference::Software,
        ..Settings::default()
    };
    let only_here = |key: &str, vrr: bool| match key {
        "check_updates" => crate::self_update::current().checks(),
        "network_priority" | "receive_coalescing" => cfg!(any(target_os = "linux", windows)),
        // It needs HDR, which hides Less color banding.
        "hdr_desktop_brightness" => false,
        // A PC's own settings, for a host that reports them (NvidiaEnhancements).
        "rtx_hdr" | "rtx_video" => false,
        "present_at_host_times" => vrr,
        "vrr_tear_free" => vrr && cfg!(windows),
        "frame_pacing" | "vsync" => !vrr,
        // Balanced's, which the default pacing is not (its own test draws it).
        "drop_late_frames" => false,
        "renderer" | "display_output" | "video_planes" | "high_performance_gpu_power" | "vulkan_video_first" | "vulkan_rfi" => cfg!(target_os = "linux"),
        "graphics_card" => graphics_card_here(),
        _ => true,
    };
    for vrr in [false, true] {
        let mut f = settings_with(Settings { vrr, ..changed.clone() });
        f.l.tearing_supported = true;
        open_folds(&mut f);
        for (at, section) in SETTINGS_SECTIONS.iter().enumerate() {
            f.l.settings_view.section = section;
            f.settle();
            let mut drawn = setting_keys(&f);
            drawn.dedup();
            let listed: Vec<&str> = SECTION_KEYS[at].iter().map(|keys| keys[0]).filter(|k| only_here(k, vrr)).collect();
            assert_eq!(drawn, listed, "{section}, VRR {vrr}");
        }
    }
}

/// The settings for developers and diagnosis sit in Debug, the last section, and in no other, on
/// the main settings and a PC's own; the search still finds them there. Advanced holds the
/// renderer, the game hint and the link and pace tests; Video position stays in Display & image.
#[test]
fn the_developer_rows_are_in_debug_and_the_tools_in_advanced() {
    // Android's main rows in its order, then PyroWave's, the decoder's and the cursor.
    const DEBUG: [&str; 13] = ["error_correction", "client_hint_bitrate", "tell_host_wireless", "tell_host_link_speed",
        "network_priority", "unbatched_input", "full_range", "pyrowave_decoder", "pyrowave_async_compute", "decoder",
        "vulkan_video_first", "vulkan_rfi", "cursor_shapes"];
    assert_eq!(SETTINGS_SECTIONS.last(), Some(&"Debug"));
    // Prioritize stream traffic does something on Linux and Windows, so only they draw it; Vulkan
    // video is Linux's; the compute queue is the GPU decoder's, macOS's default is the processor.
    let wanted = DEBUG.into_iter()
        .filter(|k| *k != "network_priority" || cfg!(any(target_os = "linux", windows)))
        .filter(|k| !matches!(*k, "vulkan_video_first" | "vulkan_rfi") || cfg!(target_os = "linux"))
        .filter(|k| *k != "pyrowave_async_compute" || Settings::default().pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu)
        .collect::<Vec<_>>();
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    let id = f.hosts.list[0].id;
    for screen in [Screen::Settings, Screen::HostSettings(id)] {
        f.l.go(screen);
        for section in SETTINGS_SECTIONS {
            if screen != Screen::Settings && section == "App" { continue; }
            f.l.settings_view.section = section;
            f.settle();
            let drawn = setting_keys(&f);
            match section {
                "Debug" => assert_eq!(drawn, wanted, "{screen:?}: Debug holds exactly these"),
                _ => assert!(!drawn.iter().any(|k| DEBUG.contains(k)), "{screen:?} {section}: {drawn:?}"),
            }
            assert_eq!(drawn.contains(&"align_video_top"), section == "Display & image", "{screen:?} {section}: {drawn:?}");
            if section == "Advanced" {
                // The renderer is Linux's choice for the whole app, so a PC's settings do not have it.
                assert_eq!(drawn.contains(&"renderer"), cfg!(target_os = "linux") && screen == Screen::Settings, "{drawn:?}");
                // So is where it shows itself, and whether a stream goes on a display plane there.
                for key in ["display_output", "video_planes", "high_performance_gpu_power"] {
                    assert_eq!(drawn.contains(&key), cfg!(target_os = "linux") && screen == Screen::Settings, "{key}: {drawn:?}");
                }
                assert!(drawn.contains(&"sops") && drawn.contains(&"link_probe"), "{screen:?}: {drawn:?}");
            }
        }
    }
    f.l.go(Screen::Settings);
    for (query, row) in [("full color range", "Full color range"), ("error correction", "Error correction"), ("local cursor", "Local cursor")] {
        f.l.settings_view.query = query.into();
        f.settle();
        // The sidebar always shows "Debug"; the section's count says the row was found there.
        assert!(shows(&f, row) && f.l.settings_view.found[SETTINGS_SECTIONS.len() - 1] > 0,
            "{query} finds {row} in Debug: {:?} {:?}", f.l.settings_view.found, f.drawn);
    }
}

/// Main settings like `settings_with`, with the AccessKit tree on.
fn settings_read_with(changed: Settings) -> Frames {
    let mut f = settings_with(changed);
    f.ctx.enable_accesskit();
    f.settle();
    f
}

/// Frame pacing and V-Sync are drawn with Variable refresh rate off, Show frames at the PC's
/// frame times with it on: a gate over some rows never carries over to the rows after them. A
/// pacing that waits for the refresh keeps V-Sync on, so its row goes; where the screen can't
/// tear, VRR has no row and works as off.
#[test]
fn the_vrr_rows_follow_their_own_gates() {
    let (vrr_row, pacing, vsync, host_times) = ("Variable refresh rate", "Frame pacing", "V-Sync", "Real VRR");
    let display = |s: Settings, tearing: bool| {
        let mut f = settings_with(s);
        f.l.tearing_supported = tearing;
        f.l.settings_view.section = "Display & image";
        open_folds(&mut f);
        f
    };
    for vrr in [false, true] {
        let f = display(Settings { vrr, frame_pacing: crate::config::FramePacing::Latency, ..Settings::default() }, true);
        assert!(shows(&f, vrr_row), "{:?}", f.drawn);
        assert_eq!((shows(&f, pacing), shows(&f, vsync), shows(&f, host_times)), (!vrr, !vrr, vrr), "VRR {vrr}: {:?}", f.drawn);
        assert!(shows(&f, "Upscaling"), "VRR {vrr}: the next group is its own");
    }
    let f = display(Settings { vrr: false, frame_pacing: crate::config::FramePacing::Balanced, ..Settings::default() }, true);
    assert!(shows(&f, pacing) && !shows(&f, vsync), "a pacing that waits keeps V-Sync on: {:?}", f.drawn);
    if cfg!(windows) {
        let f = display(Settings { vrr: true, frame_pacing: crate::config::FramePacing::Latency, ..Settings::default() }, false);
        assert!(!shows(&f, vrr_row) && shows(&f, pacing) && shows(&f, vsync) && !shows(&f, host_times), "{:?}", f.drawn);
    }
}

/// Sharper colored text is one choice for its two keys, Off or On; under PyroWave it has
/// no row, nor has Less color banding or Video decoding (in Debug).
#[test]
fn sharper_colored_text_is_one_choice_hidden_under_pyrowave() {
    let mut f = settings_with(Settings { experimental_packed_444: true, ..Settings::default() });
    open_folds(&mut f);
    assert!(row_words(&f, "Sharper colored text").contains(&"On") && shows(&f, "Less color banding"), "{:?}", f.drawn);
    click_value(&mut f, "Sharper colored text", "On");
    let commands = click_last(&mut f, "Off");
    assert!(!f.l.settings.prefer_yuv444 && !f.l.settings.experimental_packed_444, "Off turns both off");
    assert!(commands.iter().any(|c| matches!(c, Command::AskFullChromaAgain)), "{} commands", commands.len());
    click_value(&mut f, "Sharper colored text", "Off");
    click_last(&mut f, "On");
    assert!(f.l.settings.prefer_yuv444 && f.l.settings.experimental_packed_444, "On writes both");
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(shows(&f, "Video decoding"), "{:?}", f.drawn);

    let mut f = settings_with(Settings { codec: CodecPreference::PyroWave, experimental_packed_444: true, ..Settings::default() });
    open_folds(&mut f);
    assert!(!shows(&f, "Sharper colored text") && !shows(&f, "Less color banding"), "{:?}", f.drawn);
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(!shows(&f, "Video decoding") && shows(&f, "PyroWave decoder"), "{:?}", f.drawn);
}

/// A PC's own value on a row its codec hides (Sharper colored text under PyroWave) still counts;
/// with the codec given back, the row shows with its box ticked, and the box gives it back.
#[test]
fn a_pcs_own_value_on_a_hidden_row_comes_back_with_its_row() {
    use egui::accesskit::{Role, Toggled};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    f.l.settings_view.more = [true; 8];
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"codec":"PyroWave","prefer_yuv444":true}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let row_box = |f: &Frames, row: &str| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("For this PC") && n.description() == Some(row))
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
    };
    assert!(row_box(&f, "Sharper colored text").is_none() && shows(&f, "2 settings are set here."), "{:?}", f.drawn);
    let click = |f: &mut Frames, bounds: egui::accesskit::Rect| {
        f.click_at(Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32), egui::PointerButton::Primary);
    };
    let (_, codec) = row_box(&f, "Video codec").expect("the codec's box");
    click(&mut f, codec);
    let (toggled, bounds) = row_box(&f, "Sharper colored text").expect("the row is back");
    assert_eq!(toggled, Some(Toggled::True), "with the PC's own value");
    click(&mut f, bounds);
    assert!(f.l.edit_overrides.is_empty(), "{:?}", f.l.edit_overrides);
}

/// Left from any row stays in that row (its Reset, a slider) or goes to the open section's
/// entry, never to a Reset or control of another row, with changed rows above and below.
#[test]
fn left_never_lands_on_another_rows_reset() {
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let changed = [Settings { fps: 120, ..Settings::default() }, Settings { fps: 60, bitrate_kbps: 40_000, ..Settings::default() }];
    for width in [1280.0, 900.0] {
        for (case, settings) in changed.iter().enumerate() {
            for step in 0..8 {
                let mut f = settings_with(settings.clone());
                f.size = vec2(width, 4000.0);
                f.l.go(Screen::Settings);
                f.l.settings_view.section = "Video";
                f.settle();
                for _ in 0..step {
                    f.press(Key::ArrowDown);
                }
                let from = focused(&f).expect("a row's control has focus");
                let entry = f.placed.iter().filter(|(t, ..)| t == "Video").map(|p| p.1)
                    .min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
                if from.contains(entry.center()) {
                    continue;
                }
                f.press(Key::ArrowLeft);
                let to = focused(&f).expect("Left keeps a focus");
                let same_row = to.top() < from.bottom() && from.top() < to.bottom();
                assert!(to.contains(entry.center()) || same_row, "{width} case {case} step {step}: Left from {from:?} lands on {to:?}, not the entry {entry:?} or its row");
            }
        }
    }
    // A changed row's own Reset is still left of its control.
    let mut f = settings_with(Settings { width: 1280, height: 720, ..Settings::default() });
    let from = focused(&f).expect("Resolution has focus");
    f.press(Key::ArrowLeft);
    let reset = f.placed.iter().find(|(t, ..)| t == "Reset").expect("Resolution's Reset").1;
    assert!(focused(&f).is_some_and(|r| r.contains(reset.center())), "Left from {from:?} reaches the row's Reset {reset:?}, not {:?}", focused(&f));
}

/// Escape (B) in the search field leaves it for the first row found, so the D-pad carries on.
#[test]
fn escape_in_the_search_moves_focus_to_the_rows() {
    let mut f = settings_with(Settings::default());
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    f.frame(vec![egui::Event::Text("hdr".into())]);
    f.settle();
    f.press(Key::Escape);
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let results = f.placed.iter().find(|(t, ..)| t == "Results for “hdr”").expect("the results").1;
    assert_eq!(f.l.settings_view.query, "hdr", "the first Escape keeps the search");
    assert!(focused(&f).is_some_and(|r| r.left() > results.left() && r.top() > results.bottom()), "a row found has focus: {:?}", focused(&f));
    // With nothing found, the open section's entry takes it.
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    f.frame(vec![egui::Event::Text("zzzz".into())]);
    f.settle();
    f.press(Key::Escape);
    let entry = f.placed.iter().filter(|(t, ..)| t == "Video").map(|p| p.1).min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
    assert!(focused(&f).is_some_and(|r| r.contains(entry.center())), "the entry has focus: {:?}", focused(&f));
}

/// Ctrl+F is the page's: with a dialog open it leaves the focus where it is.
#[test]
fn ctrl_f_leaves_an_open_dialog_alone() {
    let mut f = settings_with(Settings::default());
    f.l.ask(Dialog::Message { title: "Network test failed".into(), body: "The PC did not answer.".into() });
    f.settle();
    let before = f.ctx.memory(|m| m.focused());
    assert!(before.is_some(), "the dialog's button has focus");
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    assert_eq!(f.ctx.memory(|m| m.focused()), before, "the dialog keeps the focus");
}

/// A PC's own value without a row on the page (beam racing) still counts, and Clear all clears it.
#[test]
fn a_pcs_value_without_a_row_counts_and_clears() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"beam_race":true}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(shows(&f, "1 setting is set here.") && shows(&f, "1 of them has no row on this page; Clear all clears it too."), "{:?}", f.drawn);
    click_text(&mut f, "Clear all for this PC");
    click_text(&mut f, "Clear all");
    assert!(f.l.edit_overrides.is_empty() && shows(&f, "Nothing is set here yet."), "{:?}", f.drawn);
}

/// Bitrate on mobile data is the profiles' cap, Android's list: on the main settings once this
/// device was on mobile data or a cap is set, and not a setting of a PC.
#[test]
fn bitrate_on_mobile_data_is_the_profiles_cap() {
    let arrays = include_str!("../../../../app/src/main/res/values/arrays.xml");
    let values = arrays.split("<string-array name=\"mobile_bitrate_values\"").nth(1).and_then(|a| a.split("</string-array>").next()).expect("Android's list");
    let android: Vec<u32> = values.split("<item>").skip(1).filter_map(|i| i.split("</item>").next()?.trim().parse().ok()).collect();
    assert_eq!(android, MOBILE_BITRATES_KBPS, "the same choices as Android");
    let mut f = settings_with(Settings::default());
    assert!(!shows(&f, "Bitrate on mobile data"), "not on mobile data, no cap: {:?}", f.drawn);
    f.l.profiles.mobile_limit_kbps = 5_000;
    f.settle();
    assert_eq!(row_words(&f, "Bitrate on mobile data"), ["5 Mbps"], "a main row with its value");
    assert!(f.l.settings_view.drawn.contains(&("mobile_bitrate", false)));
    click_value(&mut f, "Bitrate on mobile data", "5 Mbps");
    let commands = click_last(&mut f, "10 Mbps");
    assert_eq!(f.l.profiles.mobile_limit_kbps, 10_000);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveProfiles)), "{} commands", commands.len());
    assert_eq!(f.l.settings, Settings::default(), "a cap, not a setting");
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(!shows(&f, "Bitrate on mobile data"), "{:?}", f.drawn);
}

/// Controls opens the Controller buttons page from two rows, the test from its own; Controllers
/// is one choice, and with it off the controller rows have nothing to set.
#[test]
fn the_controller_rows_open_their_page_and_controllers_is_one_choice() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.section = "Controls";
    f.settle();
    for (button, test) in [("Controller buttons", false), ("Test", true)] {
        f.l.controller_test = !test;
        // The button, right of the row's label.
        let at = f.placed.iter().filter(|(t, ..)| t == button).map(|p| p.1).max_by(|a, b| a.left().total_cmp(&b.left())).expect("the button");
        let commands = f.click_at(at.center(), egui::PointerButton::Primary);
        assert!(commands.iter().any(|c| matches!(c, Command::ControllerSettings)), "{button}: {} commands", commands.len());
        assert_eq!(f.l.controller_test, test, "{button}");
        f.l.go(Screen::Settings);
        f.settle();
    }
    open_folds(&mut f);
    assert!(shows(&f, "Nintendo button layout") && shows(&f, "Stick dead zone"), "{:?}", f.drawn);
    click_value(&mut f, "Controllers", "A player each");
    click_last(&mut f, "One player");
    assert_eq!((f.l.settings.controllers, f.l.settings.multi_controller), (true, false));
    click_value(&mut f, "Controllers", "One player");
    click_last(&mut f, "Off");
    assert!(!f.l.settings.controllers, "Off sends none");
    assert!(!shows(&f, "Nintendo button layout") && !shows(&f, "Stick dead zone") && shows(&f, "Send system keys"), "{:?}", f.drawn);
}

/// When Nightjar opens is one choice for Home and the start screen; A chosen game is listed once a
/// game was chosen in its menu.
#[test]
fn when_nightjar_opens_is_one_choice() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.section = "App";
    f.settle();
    assert_eq!(row_words(&f, "When Nightjar opens"), ["Home"]);
    click_value(&mut f, "When Nightjar opens", "Home");
    assert!(shows(&f, "Show my PCs") && shows(&f, "Continue my last game") && !shows(&f, "A chosen game"), "{:?}", f.drawn);
    click_last(&mut f, "Show my PCs");
    assert_eq!((f.l.settings.open_on_start, f.l.settings.console_home), (crate::config::OpenOnStart::Pcs, false));
    f.l.open_game = Some(crate::config::OpenGame { host: "uid".into(), install_id: "a".into(), app_id: 7, title: "Racing Game".into() });
    f.settle();
    click_value(&mut f, "When Nightjar opens", "Show my PCs");
    click_last(&mut f, "Racing Game");
    assert_eq!(f.l.settings.open_on_start, crate::config::OpenOnStart::Chosen);
    assert!(row_words(&f, "When Nightjar opens").contains(&"Racing Game"), "{:?}", f.drawn);
}

/// The Controller buttons page: one switch for the test, in the rows' style, and no sentence
/// repeated under every axis and button.
#[test]
fn the_controller_buttons_page_has_one_test_switch_and_no_repeated_hints() {
    use egui::accesskit::Role;
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    f.l.go(Screen::Controllers);
    f.l.controller_key = "pad".into();
    f.l.controller_mapping = crate::gamepad::Mapping { name: "Pad".into(), ..Default::default() };
    f.settle();
    assert!(shows(&f, "Controller buttons") && shows(&f, "Controller test"), "{:?}", f.drawn);
    assert!(!f.drawn.iter().any(|t| t.starts_with("Choose the physical axis") || t.starts_with("Default follows the Nintendo") || t.starts_with("Test input")), "{:?}", f.drawn);
    let switch = f.access.as_ref().expect("AccessKit is on").nodes.iter()
        .find(|(_, n)| n.role() != Role::Label && n.label() == Some("Controller test"))
        .and_then(|(_, n)| n.bounds())
        .expect("the test's switch");
    f.click_at(Pos2::new(((switch.x0 + switch.x1) / 2.0) as f32, ((switch.y0 + switch.y1) / 2.0) as f32), egui::PointerButton::Primary);
    assert!(f.l.controller_test);
}

/// The settings page in every state worth a look, at 1280x800 (150 %) and 1920x1080: every
/// section with its main rows (`settings-section-N`) and with its fold open (`-more`, `-more-end`).
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop settings_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn settings_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    for (width, height, ppp) in [(1280.0, 800.0, 1.5), (1920.0, 1080.0, 1.0)] {
        for high in [false, true] {
            let size = format!("{}x{}{}", width as u32, height as u32, if high { "-high-contrast" } else { "" });
            let fresh = || {
                let mut f = home_with_games(1);
                f.size = vec2(width, height);
                f.native_ppp = ppp;
                f.shots = true;
                // The logo went up in frames before shots were kept; load it again so they see it.
                f.l.logo = None;
                f.l.settings.high_contrast = high;
                f.l.settings.fps = 60;
                f.l.settings.bitrate_kbps = 40_000;
                f
            };
            let shoot = |f: &mut Frames, page: &str| {
                f.settle();
                f.settle();
                f.shoot(&dir, &format!("{page}-{size}"));
            };
            let mut f = fresh();
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings");
            // A row's hint, under the pointer.
            let rate = f.placed.iter().find(|(t, ..)| t == "Frame rate").expect("the row").1.center();
            f.frame(vec![egui::Event::PointerMoved(rate)]);
            shoot(&mut f, "settings-hint");
            f.frame(vec![egui::Event::PointerGone]);
            f.l.settings_view.section = "Controls";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "settings-controls");
            if high {
                continue;
            }
            // Every section: its main rows, then its fold open, top and end.
            for (at, section) in SETTINGS_SECTIONS.iter().enumerate() {
                f.l.settings_view.section = section;
                f.l.settings_view.more = [false; 8];
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-section-{at}"));
                f.l.settings_view.more[at] = true;
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-section-{at}-more"));
                f.scroll_to_end();
                shoot(&mut f, &format!("settings-section-{at}-more-end"));
            }
            f.l.settings_view.more = [true; 8];
            // Dual screen mode on: Second screen resolution can be changed.
            f.l.settings.dual_screen = true;
            f.l.settings_view.section = "Streaming";
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-dual-screen");
            f.l.settings.dual_screen = false;
            f.l.settings_view.query = "bitrate".into();
            shoot(&mut f, "settings-search");
            f.l.settings_view.query = "teleport".into();
            shoot(&mut f, "settings-search-nothing");
            f.l.settings_view.query.clear();
            f.l.settings_view.section = "App";
            f.l.settings_view.confirm_reset = true;
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-reset-all");
            f.l.settings_view.confirm_reset = false;
            f.l.settings_view.section = "Video";
            // Presets at the defaults (Native marked), and the question a chip asks.
            let mut f = fresh();
            (f.l.settings.fps, f.l.settings.bitrate_kbps) = (0, 0);
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-presets-native");
            f.l.ask(Dialog::ConfirmPreset(Preset::BatterySaver));
            shoot(&mut f, "dialog-preset");
            f.l.dialog = Dialog::None;
            // The microphone's rows, and Balanced's Keep only the newest frame.
            f.l.settings.microphone = true;
            f.l.settings.frame_pacing = crate::config::FramePacing::Balanced;
            f.l.settings_view.more = [true; 8];
            f.l.settings_view.section = "Audio";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "settings-microphone");
            f.l.settings_view.section = "Display & image";
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-keep-newest-frame");
            // About: the version, the source link and the credits, top and end.
            let mut f = fresh();
            f.l.go(Screen::Settings);
            f.l.open(Screen::About);
            shoot(&mut f, "about");
            f.scroll_to_end();
            shoot(&mut f, "about-end");
            // The tests in the main settings: no PC that runs them, one, and two with the PC's list
            // and a saved pace.
            for count in 0..3 {
                let mut f = fresh();
                f.hosts = network_test_pcs(&[("Desk", true, Reach::Online), ("Den", true, Reach::Offline)][..count]);
                if count == 2 {
                    f.hosts.list[0].record.pace = Some(crate::config::PaceCalibration { pace_mbps: 180, client_link_mbps: 1000, host_link_mbps: 2500, wifi: false, ssid: None, qualified: true });
                }
                f.l.go(Screen::Settings);
                f.l.settings_view.more = [true; 8];
                f.l.settings_view.section = "Streaming";
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-connection-test-{count}"));
                f.l.settings_view.section = "Advanced";
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-frame-pace-{count}"));
            }
            // Bitrate on mobile data, with a cap set on the Profiles page.
            let mut f = fresh();
            f.l.profiles.mobile_limit_kbps = 10_000;
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-mobile-bitrate");
            // A PC's settings with two of its own values, over a profile.
            let mut f = fresh();
            let pc = f.hosts.list[0].id;
            let travel = f.l.profiles.create("Travel", serde_json::from_str(r#"{"bitrate_kbps":20000}"#).unwrap()).unwrap();
            f.l.profiles.active = Some(travel);
            f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"fps":120,"hdr":true}"#).unwrap());
            f.l.open_host_settings(pc, &f.hosts.list[0]);
            shoot(&mut f, "pc-settings");
            f.l.settings_view.section = "Debug";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "pc-settings-debug");
            f.l.settings_view.section = "Video";
            // A game's settings: one of its own, the rest from the PC, the profile and the main settings.
            let app = nightjar_core::http::App { name: "Racing Game".into(), uuid: "install-0".into(), ..game(1000) };
            f.l.open_app_settings(pc, &f.hosts.list[0], app);
            f.l.edit_overrides.capture(&Settings { codec: CodecPreference::Hevc, ..Settings::default() }, &["codec"]);
            f.l.edit_settings = f.l.edit_overrides.apply(&f.l.inherited_settings);
            shoot(&mut f, "game-settings");
            // The main settings while a profile is in use.
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-profile-in-use");
        }
    }
}

/// These PCs, each paired or not and answering or not, every one with both network tests.
fn network_test_pcs(pcs: &[(&str, bool, Reach)]) -> Hosts {
    let answer = nightjar_core::http::ServerInfo::parse("<root status_code=\"200\"><uniqueid>uid</uniqueid><state>SUNSHINE_SERVER_FREE</state>\
        <PairStatus>1</PairStatus><RustHostVersion>3.0.0</RustHostVersion><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes>\
        <PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion>\
        <PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion></root>", 47989).expect("the answer parses");
    let mut hosts = Hosts::without_discovery(pcs.iter().map(|(name, paired, _)| record(name, *paired, false)).collect(), Arc::new(|| {}));
    for (host, (.., reach)) in hosts.list.iter_mut().zip(pcs) {
        (host.reach, host.info) = (*reach, Some(answer.clone()));
    }
    hosts
}

/// The main settings with `pcs` (see `network_test_pcs`), every fold open, on a tall window.
fn settings_with_pcs(pcs: &[(&str, bool, Reach)]) -> Frames {
    let mut f = settings_read_with(Settings::default());
    f.hosts = network_test_pcs(pcs);
    open_folds(&mut f);
    f
}

fn in_section(f: &mut Frames, section: &'static str) {
    f.l.settings_view.section = section;
    f.settle();
}

/// A customer looked for the PyroWave calibration in Settings: Connection test is one of
/// Streaming's main rows, the frame pace is under Advanced › Diagnostics, as on a PC's own
/// settings, and the search finds both.
#[test]
fn the_connection_test_and_frame_pace_are_in_the_main_settings_and_a_pcs_own() {
    let mut f = settings_with_pcs(&[("Desk", true, Reach::Online)]);
    let id = f.hosts.list[0].id;
    for page in [Screen::Settings, Screen::HostSettings(id)] {
        if page != Screen::Settings {
            f.l.open_host_settings(id, &f.hosts.list[0]);
        }
        in_section(&mut f, "Streaming");
        assert!(f.l.settings_view.drawn.contains(&("connection_test", false)), "{page:?}: a main row: {:?}", f.l.settings_view.drawn);
        // One PC is the PC: nothing to choose. An action: no box of a PC's own, no Reset.
        assert_eq!(row_words(&f, "Connection test"), ["Test"], "{page:?}");
        let commands = click_text(&mut f, "Test");
        assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == id), "{page:?}: {} commands", commands.len());
        in_section(&mut f, "Advanced");
        assert!(shows(&f, "Diagnostics"), "{page:?}: {:?}", f.drawn);
        assert_eq!(row_words(&f, "PyroWave frame pace"), ["Calibrate"], "{page:?}: nothing to clear before a calibration");
        let commands = click_text(&mut f, "Calibrate");
        assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: true }] if host == id), "{page:?}: {} commands", commands.len());
    }
    // A saved pace says so and can be cleared, from the main settings too.
    f.hosts.list[0].record.pace = Some(crate::config::PaceCalibration { pace_mbps: 180, client_link_mbps: 1000, host_link_mbps: 2500, wifi: false, ssid: None, qualified: true });
    f.l.go(Screen::Settings);
    in_section(&mut f, "Advanced");
    assert!(shows(&f, "180 Mbps · confirmed"), "{:?}", f.drawn);
    let commands = click_text(&mut f, "Clear");
    assert!(matches!(commands[..], [Command::ClearPace(host)] if host == id), "{} commands", commands.len());

    let at = |name: &str| SETTINGS_SECTIONS.iter().position(|s| *s == name).unwrap();
    for (query, row, section) in [("connection test", "Connection test", "Streaming"), ("download speed", "Connection test", "Streaming"),
        ("pyrowave frame pace", "PyroWave frame pace", "Advanced"), ("diagnostics", "PyroWave frame pace", "Advanced")] {
        f.l.settings_view.query = query.into();
        f.settle();
        assert!(shows(&f, row) && f.l.settings_view.found[at(section)] > 0, "{query}: {:?} {:?}", f.l.settings_view.found, f.drawn);
    }
    f.l.settings_view.query = "pyrowave frame pace".into();
    f.settle();
    assert!(!shows(&f, "Connection test"), "{:?}", f.drawn);
}

/// With several paired PCs a list beside each test chooses the one tested: the first that is
/// online until the user picks another, never one that is not paired. One off the network says so.
#[test]
fn the_main_settings_tests_choose_among_several_paired_pcs() {
    let mut f = settings_with_pcs(&[("Loft", false, Reach::Online), ("Desk", true, Reach::Offline), ("Den", true, Reach::Online)]);
    let (loft, desk, den) = (f.hosts.list[0].id, f.hosts.list[1].id, f.hosts.list[2].id);
    in_section(&mut f, "Streaming");
    let words = row_words(&f, "Connection test");
    assert!(words.contains(&"Test") && words.contains(&"Den"), "the paired PC that is online: {words:?}");
    assert_eq!(f.l.settings_view.test_pc, None, "nothing is chosen until the user does");
    let commands = click_text(&mut f, "Test");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == den), "{} commands", commands.len());

    // The list has the paired PCs only; choosing one moves both tests to it.
    click_text(&mut f, "Den");
    assert!(shows(&f, "Desk") && !shows(&f, "Loft"), "{:?}", f.drawn);
    click_last(&mut f, "Desk");
    assert_eq!(f.l.settings_view.test_pc, Some(desk));
    // It is off the network: both tests wait for it.
    let words = row_words(&f, "Connection test");
    assert!(words.contains(&"Desk") && words.contains(&"Offline") && !words.contains(&"Test"), "{words:?}");
    in_section(&mut f, "Advanced");
    let words = row_words(&f, "PyroWave frame pace");
    assert!(words.contains(&"Desk") && words.contains(&"Offline") && !words.contains(&"Calibrate"), "{words:?}");
    f.hosts.list[1].reach = Reach::Online;
    f.settle();
    let commands = click_text(&mut f, "Calibrate");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: true }] if host == desk), "{} commands", commands.len());

    // The choice is the launcher's, not a setting, and holds only while that PC is listed.
    assert_eq!(f.l.settings, Settings::default());
    let pc = |f: &Frames, chosen| network_test_pc(&test_pcs(Screen::Settings, &f.hosts, Host::bandwidth_test), chosen).map(|h| h.id);
    assert_eq!(pc(&f, Some(den)), Some(den));
    assert_eq!(pc(&f, Some(loft)), Some(desk), "not paired: the first paired PC that is online");
    assert_eq!(pc(&f, Some(u64::MAX)), Some(desk), "forgotten");
    f.hosts.list[1].reach = Reach::Offline;
    f.hosts.list[2].reach = Reach::Unknown;
    assert_eq!(pc(&f, None), Some(desk), "none online: the first paired one");

    // One paired PC beside one that is not: nothing to choose.
    let mut f = settings_with_pcs(&[("Loft", false, Reach::Online), ("Desk", true, Reach::Online)]);
    in_section(&mut f, "Streaming");
    assert_eq!(row_words(&f, "Connection test"), ["Test"], "{:?}", f.drawn);
}

/// The tests are drawn only with a paired PC that runs them: none without one; the rows around
/// them stay. Connection test measures ping, jitter and loss on any paired PC, so a PC whose host
/// has no speed test has its row (and the delay alone), as on Android.
#[test]
fn the_tests_have_no_row_without_a_pc_that_runs_them() {
    for pcs in [&[][..], &[("Loft", false, Reach::Online)][..]] {
        let mut f = settings_with_pcs(pcs);
        in_section(&mut f, "Streaming");
        assert!(!shows(&f, "Connection test") && shows(&f, "Recover interrupted streams") && shows(&f, "Keep streaming when the network changes"), "{:?}", f.drawn);
        in_section(&mut f, "Advanced");
        assert!(!shows(&f, "PyroWave frame pace") && shows(&f, "Test the link before starting"), "{:?}", f.drawn);
        assert!(test_pcs(Screen::Settings, &f.hosts, Host::bandwidth_test).is_empty());
    }
    // A paired PC that never said it has a speed test (stock hosts): the delay test is there.
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    in_section(&mut f, "Streaming");
    assert_eq!(row_words(&f, "Connection test"), ["Test"], "{:?}", f.drawn);
    let id = f.hosts.list[0].id;
    let commands = click_text(&mut f, "Test");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == id), "{} commands", commands.len());
    in_section(&mut f, "Advanced");
    assert!(!shows(&f, "PyroWave frame pace"), "no calibration without PyroWave: {:?}", f.drawn);
}

/// Nightjar #87: system keys can go to the PC only while the stream is full screen. Off as
/// before, a setting of its own right after Send system keys, and no row with that off.
#[test]
fn system_keys_only_in_full_screen_sits_with_send_system_keys() {
    let defaults = Settings::default();
    assert!(defaults.capture_system_keys && !defaults.system_keys_fullscreen_only, "as before: in a window too");
    let old: Settings = serde_json::from_str(r#"{"capture_system_keys":false}"#).unwrap();
    assert!(!old.capture_system_keys && !old.system_keys_fullscreen_only, "a settings file from before");
    let saved = serde_json::to_value(Settings { system_keys_fullscreen_only: true, ..Settings::default() }).unwrap();
    assert_eq!((saved["capture_system_keys"].as_bool(), saved["system_keys_fullscreen_only"].as_bool()), (Some(true), Some(true)));
    for send in [true, false] {
        let mut f = settings_with(Settings { capture_system_keys: send, ..Settings::default() });
        f.l.settings_view.section = "Controls";
        open_folds(&mut f);
        assert!(shows(&f, "Send system keys") && shows(&f, "Right Alt as Windows key"), "{:?}", f.drawn);
        assert_eq!(shows(&f, "System keys only in full screen"), send, "Send system keys {send}: {:?}", f.drawn);
        let rows = setting_keys(&f);
        let at = rows.iter().position(|key| *key == "capture_system_keys").unwrap();
        assert_eq!(rows[at + 1], if send { "system_keys_fullscreen_only" } else { "right_alt_as_meta" }, "right after Send system keys");
    }
}

/// Runs `draw` in a bare frame `width` wide.
fn bare_frame<R>(width: f32, draw: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = egui::Context::default();
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 800.0))), ..Default::default() };
    let (mut draw, mut out) = (Some(draw), None);
    let mut output = ctx.run_ui(input, |ui| {
        if let Some(draw) = draw.take() {
            out = Some(draw(ui));
        }
    });
    output.textures_delta.clear();
    out.expect("the frame ran")
}

/// Rows placed one after another are one group across the whole column; a gap starts another.
#[test]
fn rows_that_follow_each_other_are_one_group_across_the_column() {
    let (a, b, c, column_width) = bare_frame(1000.0, |ui| {
        let column_width = ui.available_width();
        let a = group_row(ui, |ui| { ui.label("a"); });
        let b = group_row(ui, |ui| { ui.label("b"); });
        ui.add_space(30.0);
        let c = group_row(ui, |ui| { ui.label("c"); });
        (a, b, c, column_width)
    });
    assert!((a.bottom() - b.top()).abs() < 0.5, "{a:?} then {b:?}");
    assert!((a.width() - column_width).abs() < 0.5, "{} of {column_width}", a.width());
    assert!(c.top() - b.bottom() >= 30.0, "{b:?} then {c:?}");
}

/// A column as wide as a page may ever want still ends with the page: its controls stay on screen.
#[test]
fn a_column_ends_with_the_page_and_stops_at_its_widest_on_a_wide_window() {
    for (window, most) in [(800.0, 800.0), (3000.0, SETTINGS_W)] {
        let right = bare_frame(window, |ui| {
            let mut right = 0.0;
            column(ui, SETTINGS_W, |ui| right = group_row(ui, |ui| { ui.label("a"); }).right());
            right
        });
        assert!(right <= most + 8.0 && right > most - 60.0, "at {window}: {right}");
    }
}

/// A group's heading is the section type as written, not a shouted label, and no group is a
/// catch-all "Fine-tuning".
#[test]
fn a_group_heading_is_written_as_it_reads() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.section = "Display & image";
    open_folds(&mut f);
    assert!(shows(&f, "Refresh rate") && shows(&f, "PC display"), "{:?}", f.drawn);
    assert!(!shows(&f, "REFRESH RATE") && !shows(&f, "Fine-tuning"), "{:?}", f.drawn);
}

/// The switch and the list choice take the sizes the design gives every control.
#[test]
fn a_switch_and_a_list_choice_have_the_designed_size() {
    let (switch, choice) = bare_frame(1000.0, |ui| {
        let switch = toggle(ui, &mut true).rect;
        let choice = Combo::from_id_salt("choice").selected_text("Automatic").show_ui(ui, |_| {}).response.rect;
        (switch, choice)
    });
    assert_eq!(switch.size(), vec2(design_tokens::SIZE_SWITCH_W, design_tokens::SIZE_SWITCH_H));
    assert_eq!(choice.height(), 40.0);
}

/// The Graphics card row is drawn only on a Windows PC with two GPUs.
fn graphics_card_here() -> bool {
    #[cfg(windows)]
    {
        crate::gpu_adapters::gpus().is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Presets sit above the main settings' rows: the defaults read Native, a chip asks before it
/// replaces the stream settings and sets them on OK, and a changed value reads Custom. A PC's
/// settings and a search have none.
#[test]
fn presets_ask_first_set_their_values_and_read_custom_once_changed() {
    let mut f = settings_with(Settings::default());
    assert!(shows(&f, "Presets") && shows(&f, "Native (recommended)") && shows(&f, "Battery saver"), "{:?}", f.drawn);
    assert!(shows(&f, "Native"), "the defaults are Native: {:?}", f.drawn);
    click_text(&mut f, "Battery saver");
    assert!(matches!(f.l.dialog, Dialog::ConfirmPreset(Preset::BatterySaver)));
    assert!(shows(&f, "This replaces resolution, frame rate, bitrate, frame pacing, upscaling, VRR and performance settings with this preset’s values. Codec and HDR choices stay as they are."));
    click_text(&mut f, "Cancel");
    assert!(matches!(f.l.dialog, Dialog::None) && f.l.settings == Settings::default(), "Cancel changes nothing");
    f.l.settings.codec = CodecPreference::Hevc;
    f.l.settings.hdr = true;
    f.settle();
    click_text(&mut f, "Battery saver");
    let commands = click_text(&mut f, "OK");
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "saved at once");
    let s = &f.l.settings;
    assert_eq!((s.width, s.height, s.fps, s.bitrate_kbps, s.frame_pacing), (0, 0, 30, 0, crate::config::FramePacing::CapFps));
    assert!(s.codec == CodecPreference::Hevc && s.hdr, "codec and HDR stay");
    f.settle();
    assert!(shows(&f, "Battery saver") && !shows(&f, "Custom"), "{:?}", f.drawn);
    f.l.settings.bitrate_kbps = 20_000;
    f.settle();
    assert!(shows(&f, "Custom"), "{:?}", f.drawn);
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    assert!(!shows(&f, "Presets"), "a search shows rows only");
    f.l.settings_view.query.clear();
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(!shows(&f, "Presets"), "a PC's settings have none");
}

/// Native puts back what the presets set, the desktop's defaults among them, and turns on every
/// latency switch the desktop has; Battery saver leaves those. VRR the screen can't use never
/// makes a preset Custom, as on Android.
#[test]
fn a_preset_sets_and_reads_the_desktops_settings() {
    let mut s = Settings {
        width: 1280, height: 720, fps: 144, bitrate_kbps: 50_000, vrr: false,
        upscaling: crate::config::Upscaling::Fsr1, frame_pacing: crate::config::FramePacing::Smoothness,
        drop_late_frames: false, unbatched_input: false, network_priority: false,
        codec: CodecPreference::Av1, prefer_yuv444: true,
        ..Settings::default()
    };
    assert_eq!(presets::chosen(&preset_current(&s, true)), None);
    apply_preset(&mut s, Preset::Native);
    assert_eq!(presets::chosen(&preset_current(&s, true)), Some(Preset::Native));
    assert_eq!(s, Settings { codec: CodecPreference::Av1, prefer_yuv444: true, ..Settings::default() }, "the rest are the defaults");
    assert_eq!(presets::chosen(&preset_current(&Settings::default(), true)), Some(Preset::Native), "the defaults are Native");
    s.unbatched_input = false;
    apply_preset(&mut s, Preset::BatterySaver);
    assert_eq!(presets::chosen(&preset_current(&s, true)), Some(Preset::BatterySaver));
    assert!(!s.unbatched_input, "Battery saver leaves the latency switches");
    s.vrr = false;
    assert_eq!(presets::chosen(&preset_current(&s, true)), None, "VRR off where it is offered is Custom");
    assert_eq!(presets::chosen(&preset_current(&s, false)), Some(Preset::BatterySaver), "VRR the screen lacks reads as its default");
    assert_eq!(preset_value(Preset::Native), "Native");
    assert_eq!(preset_value(Preset::BatterySaver), "Battery saver");
}

/// Keep only the newest frame is Balanced's choice: it shows with Balanced frame pacing and VRR
/// off, after Frame pacing, and not for the pacings that decide it themselves.
#[test]
fn keep_only_the_newest_frame_shows_for_balanced_pacing_only() {
    let row = "Keep only the newest frame";
    for (pacing, vrr, shown) in [
        (crate::config::FramePacing::Balanced, false, true),
        (crate::config::FramePacing::Balanced, true, false),
        (crate::config::FramePacing::Latency, false, false),
        (crate::config::FramePacing::CapFps, false, false),
        (crate::config::FramePacing::Smoothness, false, false),
    ] {
        let mut f = settings_with(Settings { frame_pacing: pacing, vrr, ..Settings::default() });
        f.l.tearing_supported = true;
        open_folds(&mut f);
        f.l.settings_view.section = "Display & image";
        f.settle();
        assert_eq!(shows(&f, row), shown, "{pacing:?} VRR {vrr}: {:?}", f.drawn);
        if shown {
            let keys = setting_keys(&f);
            let at = keys.iter().position(|k| *k == "frame_pacing").unwrap();
            assert_eq!(keys.get(at + 1), Some(&"drop_late_frames"), "{keys:?}");
            assert!(!shows(&f, "V-Sync"), "Balanced keeps V-Sync on");
        }
    }
}

/// Noise suppression and Automatic gain follow the microphone, after Echo cancellation.
#[test]
fn the_microphones_noise_suppression_and_gain_follow_it() {
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    f.l.settings_view.section = "Audio";
    f.settle();
    assert!(!shows(&f, "Noise suppression") && !shows(&f, "Automatic gain"), "{:?}", f.drawn);
    f.l.settings.microphone = true;
    f.settle();
    let keys = setting_keys(&f);
    let at = keys.iter().position(|k| *k == "mic_echo_cancel").unwrap();
    assert_eq!(&keys[at + 1..at + 3], ["mic_noise_suppression", "mic_auto_gain"], "{keys:?}");
    assert!(Settings::default().mic_noise_suppression && !Settings::default().mic_auto_gain, "Android's defaults");
}

/// App: Theme first; Check for updates now where this copy checks; Use small box art is the
/// card size; About opens its page. Advanced: the decode time, the latency benchmark and the
/// latency CSV, on the main settings only.
#[test]
fn the_app_and_diagnostics_rows_from_android_do_what_they_say() {
    let mut f = settings_read_with(Settings::default());
    open_folds(&mut f);
    f.l.settings_view.section = "App";
    f.settle();
    assert_eq!(setting_keys(&f).first(), Some(&"theme"), "Theme comes first");
    assert_eq!(row_words(&f, "Theme"), ["Dark"]);
    assert_eq!(shows(&f, "Check for updates now"), crate::self_update::current().checks());
    if crate::self_update::current().checks() {
        let commands = click_text(&mut f, "Check now");
        assert!(matches!(commands[..], [Command::CheckUpdatesNow]), "{} commands", commands.len());
    }
    assert_eq!(f.l.settings.home_card_size, crate::config::CardSize::Medium);
    let small_box_art = |f: &Frames| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.label() == Some("Use small box art") && n.toggled().is_some())
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
            .expect("the Use small box art switch")
    };
    let (toggled, bounds) = small_box_art(&f);
    assert_eq!(toggled, Some(egui::accesskit::Toggled::False));
    let at = Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    f.click_at(at, egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_card_size, crate::config::CardSize::Small, "on is Home's Small cards");
    f.click_at(at, egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_card_size, crate::config::CardSize::Medium, "off is Medium");
    f.l.settings.home_card_size = crate::config::CardSize::Large;
    f.settle();
    assert_eq!(small_box_art(&f).0, Some(egui::accesskit::Toggled::False), "Large is not small");
    f.l.settings.home_card_size = crate::config::CardSize::Medium;
    f.settle();
    let about = f.placed.iter().find(|(t, ..)| t == "About").expect("the About row").1;
    let open = f.placed.iter().find(|(t, r, _)| t == "Open" && (r.center().y - about.center().y).abs() < 24.0).expect("its button").1.center();
    f.click_at(open, egui::PointerButton::Primary);
    f.settle();
    assert_eq!(f.l.screen, Screen::About);
    assert!(shows(&f, &format!("Version {} · Based on Moonlight (GPL-3.0)", crate::VERSION)) && shows(&f, "Source and licenses"), "{:?}", f.drawn);
    assert!(shows(&f, "Moonlight Android") && shows(&f, "Cameron Gutman, Diego Waxemberg, Aaron Neyer, Andrew Hennessy and contributors · GPL-3.0"), "{:?}", f.drawn);
    let commands = click_text(&mut f, "Source and licenses");
    assert!(matches!(commands[..], [Command::OpenSource]), "{} commands", commands.len());
    f.l.back(&mut Vec::new(), false);
    f.settle();
    assert_eq!(f.l.screen, Screen::Settings);

    f.l.settings_view.section = "Advanced";
    f.settle();
    assert!(setting_keys(&f).contains(&"post_stream_decode_time"), "{:?}", setting_keys(&f));
    let commands = click_text(&mut f, "Export");
    assert!(matches!(commands[..], [Command::ExportLatencyCsv]), "{} commands", commands.len());
    let bench = f.placed.iter().find(|(t, ..)| t == "Latency benchmark").expect("the row").1;
    let open = f.placed.iter().find(|(t, r, _)| t == "Open" && (r.center().y - bench.center().y).abs() < 24.0).expect("its button").1.center();
    f.click_at(open, egui::PointerButton::Primary);
    f.settle();
    assert_eq!(f.l.screen, Screen::LatencyBenchmark);

    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.l.settings_view.section = "Advanced";
    f.settle();
    assert!(!shows(&f, "Show decode time after streaming") && !shows(&f, "Latency benchmark") && !shows(&f, "Export latency CSV"), "{:?}", f.drawn);
}
