//! The settings page as people use it: sections and their folds, search, reset to default and the
//! PC and game layers, driven through whole frames like the launcher's other tests.

use super::*;
use crate::ui::settings::{simple_apply, simple_caps, simple_state};
use nightjar_policy::simple_profile as simple;
use nightjar_policy::choices::LowerBitrate;
use crate::config::FramePacing;
use nightjar_policy::presets::{self, Preset};

/// Main settings at their defaults but `changed`, on a tall window where a section never scrolls,
/// with "More settings" chosen and Video open: the whole list of sections and the rows most of
/// these tests are about. `fresh_settings` is the page as a first visit finds it.
fn settings_with(changed: Settings) -> Frames {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.l.settings = changed;
    f.l.settings_view.rail_more = true;
    f.l.settings_view.section = "Video";
    f.l.go(Screen::Settings);
    f.settle();
    f
}

/// Main settings as a first visit finds them, at `width`.
fn fresh_settings(width: f32) -> Frames {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(width, 4000.0);
    f.l.go(Screen::Settings);
    f.settle();
    f
}

/// The entries of the list beside the rows (chips on a narrow window), top to bottom and left to
/// right: the sections, Profiles and "More settings". The page's own title is not one of them.
fn rail_entries(f: &Frames) -> Vec<&str> {
    let named = |t: &str| SETTINGS_SECTIONS.contains(&t) || t == "Profiles" || t == "More settings";
    let small = f.placed.iter().filter(|(t, ..)| named(t)).map(|p| p.1.height()).fold(f32::MAX, f32::min);
    // The title is the open section's name again, in the page title's size.
    let mut entries: Vec<(&str, Rect)> = f.placed.iter().filter(|(t, r, _)| named(t) && r.height() < small * 1.3).map(|(t, r, _)| (t.as_str(), *r)).collect();
    entries.sort_by(|a, b| (a.1.top(), a.1.left()).partial_cmp(&(b.1.top(), b.1.left())).unwrap());
    entries.dedup_by(|a, b| a.0 == b.0);
    entries.into_iter().map(|e| e.0).collect()
}

const RAIL_SIMPLE: [&str; 5] = ["Recommended", "Controls", "Audio", "App", "More settings"];
/// The list unfolded at the defaults: Debug is listed only on request
/// (`debug_is_listed_on_request_or_while_a_debug_setting_is_changed`).
const RAIL_ALL: [&str; 10] = ["Recommended", "Controls", "Audio", "App", "More settings", "Profiles", "Video", "Display & image", "Streaming", "Advanced"];

/// Simple first: a first visit opens Recommended and lists the four sections most people need;
/// More settings unfolds Profiles and the sections of detail under them, and folds them again.
#[test]
fn the_list_is_simple_first_and_more_settings_unfolds_the_rest() {
    for width in [1280.0, 800.0] {
        let mut f = fresh_settings(width);
        assert_eq!(f.l.settings_view.section, "Recommended", "at {width}");
        assert!(shows(&f, "Stream smoothness"), "{:?}", f.drawn);
        assert_eq!(rail_entries(&f), RAIL_SIMPLE, "at {width}");
        click_text(&mut f, "More settings");
        assert_eq!(rail_entries(&f), RAIL_ALL, "at {width}");
        click_text(&mut f, "Display & image");
        assert!(f.l.settings_view.section == "Display & image" && !shows(&f, "Stream smoothness"), "{:?}", f.drawn);
        // Folding while a section behind the fold is open goes back to the first section: the list
        // never hides what the rows show.
        click_text(&mut f, "More settings");
        assert_eq!(rail_entries(&f), RAIL_SIMPLE, "at {width}");
        assert_eq!(f.l.settings_view.section, "Recommended", "at {width}");
    }
}

/// More settings stays as the user left it across restarts, as on Android: choosing it saves the
/// choice and a launcher started from the saved settings opens with it unfolded. A section behind
/// the fold that a link or a search shows unfolds it for that visit and saves nothing.
#[test]
fn more_settings_is_remembered_but_a_link_or_search_unfold_is_not() {
    let mut f = fresh_settings(1280.0);
    let commands = click_text(&mut f, "More settings");
    assert!(f.l.settings.settings_more_open, "the choice is kept in settings.json");
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "and saved");
    let saved = serde_json::to_string(&f.l.settings).unwrap();
    let mut again = Frames::new(record("PC", true, false), Reach::Online);
    again.size = vec2(1280.0, 4000.0);
    again.l = launcher(false, false);
    again.l.settings = serde_json::from_str(&saved).unwrap();
    again.l.settings_view = crate::ui::settings::SettingsView::remembered(again.l.settings.settings_more_open);
    again.l.go(Screen::Settings);
    again.settle();
    assert_eq!(rail_entries(&again), RAIL_ALL, "a new start opens with it unfolded");
    assert!(Launcher::new(again.l.settings.clone(), Profiles::default(), crate::shortcuts::Shortcuts::default(), again.l.display).settings_view.rail_more, "Launcher::new reads it");
    // Folding it again is remembered too.
    click_text(&mut again, "More settings");
    assert!(!again.l.settings.settings_more_open);
    assert_eq!(rail_entries(&again), RAIL_SIMPLE);

    // A link to a section behind the fold, and a search: shown for the visit, nothing saved.
    let mut f = fresh_settings(1280.0);
    f.l.settings_view.section = "Streaming";
    let mut commands = f.settle();
    assert_eq!(rail_entries(&f), RAIL_ALL);
    f.l.settings_view.query = "bitrate".into();
    commands.extend(f.settle());
    f.l.settings_view.query.clear();
    commands.extend(f.settle());
    assert!(!f.l.settings.settings_more_open && !f.l.settings_view.rail_more, "an unfold for a visit is not the user's choice");
    assert!(!commands.iter().any(|c| matches!(c, Command::SaveSettings)), "nothing is saved for it");
    // Reset all leaves it as it is: it is no setting.
    let kept = Settings { settings_more_open: true, setup_card_host: Some(7), setup_card_seen: true, fps: 120, ..Settings::default() };
    let reset = crate::ui::settings::reset_to_defaults(&kept);
    assert_eq!((reset.fps, reset.settings_more_open, reset.setup_card_host, reset.setup_card_seen), (Settings::default().fps, true, Some(7), true));
}

/// Nothing is out of reach behind the fold: a section opened from elsewhere shows with its entry,
/// a search lists every section with its count, and a controller walks into the fold.
#[test]
fn a_section_behind_more_settings_shows_with_its_entry() {
    let mut f = fresh_settings(1280.0);
    f.l.settings_view.section = "Streaming";
    f.settle();
    assert_eq!(rail_entries(&f), RAIL_ALL);
    let mut f = fresh_settings(1280.0);
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    let listed = rail_entries(&f);
    assert!(listed.contains(&"Video") && !listed.contains(&"More settings"), "a search looks in every section: {listed:?}");
    f.l.settings_view.query.clear();
    f.settle();
    assert_eq!(rail_entries(&f), RAIL_SIMPLE, "the search over, the list is as it was");
    // The D-pad: from the rows left to the list, down to More settings, A unfolds it, down again
    // reaches Profiles.
    let mut f = fresh_settings(1280.0);
    f.press(Key::ArrowLeft);
    assert!(focus_to(&mut f, Key::ArrowDown, "More settings"), "Down reaches More settings");
    f.press(Key::Enter);
    assert_eq!(rail_entries(&f), RAIL_ALL);
    assert!(focus_to(&mut f, Key::ArrowDown, "Profiles"), "Down walks into what it unfolded");
}

/// A PC's settings open on Video; the main settings, opened after them, are simple again.
#[test]
fn a_pcs_settings_do_not_unfold_the_main_list() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert_eq!(f.l.settings_view.section, "Video");
    f.l.go(Screen::Settings);
    f.settle();
    assert_eq!(f.l.settings_view.section, "Recommended");
    assert_eq!(rail_entries(&f), RAIL_SIMPLE);
    // The user's own choice stays: More settings and Video, away and back.
    click_text(&mut f, "More settings");
    click_text(&mut f, "Video");
    f.l.go(Screen::Home);
    f.settle();
    f.l.go(Screen::Settings);
    f.settle();
    assert_eq!(f.l.settings_view.section, "Video");
    assert_eq!(rail_entries(&f), RAIL_ALL);
}

/// A PC's, game's or profile's settings are the detail already: every section they have, no fold.
#[test]
fn a_pcs_settings_list_every_section_without_a_fold() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    // Debug by the same rule as Settings (`a_pcs_settings_list_debug_by_the_same_rule`).
    assert_eq!(rail_entries(&f), ["Video", "Display & image", "Controls", "Audio", "Streaming", "Advanced"]);
}

/// Opens every section's fold, as its "More … settings" row does.
fn open_folds(f: &mut Frames) {
    f.l.settings_view.more = [true; 9];
    f.settle();
}

/// An open section's rows stay in the card under its main rows, and the fold row is its last line:
/// "Fewer … settings" sits under every row it opened.
#[test]
fn an_open_sections_fold_row_is_its_last_line() {
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    let words = fold_words(&f).expect("Video has a fold");
    assert!(words.starts_with("Fewer "), "{words}");
    let fold = f.placed.iter().find(|(t, ..)| *t == words).map(|p| p.1).unwrap();
    let below: Vec<&String> = f.placed.iter().filter(|(t, r, _)| *t != words && (r.left() - fold.left()).abs() < 30.0 && r.top() > fold.bottom()).map(|p| &p.0).collect();
    assert!(below.is_empty(), "nothing of the section under its fold row: {below:?}");
    assert!(f.l.settings_view.drawn.iter().any(|(_, folded)| *folded), "the opened rows are drawn");
}

/// The open section's fold row as the last frame wrote it, if it has one.
fn fold_words(f: &Frames) -> Option<String> {
    f.drawn.iter().find(|t| (t.starts_with("More ") || t.starts_with("Fewer ")) && t.contains(" settings") && *t != "More settings").cloned()
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
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    for page in [Screen::Settings, Screen::HostSettings(id)] {
        if page == Screen::Settings { f.l.go(page) } else { f.l.open_host_settings(id, &f.hosts.list[0]) }
        // Video is behind More settings on the main settings; a PC's own open on it.
        (f.l.settings_view.rail_more, f.l.settings_view.section) = (true, "Video");
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
    let video = SETTINGS_SECTIONS.iter().position(|s| *s == "Video").unwrap();
    assert_eq!(f.l.settings_view.more, [false; 9]);
    assert!(f.l.settings_view.drawn.iter().all(|(_, folded)| !folded), "only main rows: {:?}", f.l.settings_view.drawn);
    assert!(!shows(&f, "Sharper colored text"), "{:?}", f.drawn);
    let more = fold_words(&f).expect("Video has a fold");
    click_text(&mut f, &more);
    assert!(f.l.settings_view.more[video] && shows(&f, "Fewer Video settings") && shows(&f, "Sharper colored text"), "{:?}", f.drawn);
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
    assert!(!f.l.settings_view.more[video] && !shows(&f, "Sharper colored text"), "{:?}", f.drawn);
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
    assert!(y(&f, "Video codec") >= below - 0.5, "the rows below never move up");
    // Under the pointer no row changes size, however narrow the room: the hint keeps one line,
    // cut (its whole text is the tooltip), and the rows below stay where they are.
    f.size = vec2(980.0, 4000.0);
    f.settle();
    let below = y(&f, "Video codec");
    let rate = f.placed.iter().find(|(t, ..)| t == "Frame rate").expect("the row").1;
    f.frame(vec![egui::Event::PointerMoved(rate.center())]);
    f.settle();
    let hint = f.placed.iter().find(|(t, ..)| t == rate_hint).expect("the hint").1;
    assert!(hint.height() < 24.0, "one line: {hint:?}");
    assert!(hint_elided(&f, rate_hint), "cut under a pointer");
    assert!((y(&f, "Video codec") - below).abs() < 0.5, "no row moves under the pointer");
}

/// Whether the galley of `text` the last frame drew was cut short with "…".
fn hint_elided(f: &Frames, text: &str) -> bool {
    fn walk(shape: &egui::Shape, text: &str, out: &mut Option<bool>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text, out)),
            egui::Shape::Text(t) if t.galley.job.text == text => *out = Some(t.galley.elided),
            _ => {}
        }
    }
    let mut out = None;
    f.shapes.iter().for_each(|c| walk(&c.shape, text, &mut out));
    out.unwrap_or_else(|| panic!("{text} is not drawn"))
}

/// At 1280 x 800, as the screenshots were taken, the focused Resolution row shows its whole hint,
/// the "when to change it" clause included.
#[test]
fn the_focused_rows_hint_is_drawn_whole_at_1280_by_800() {
    let mut f = settings_with(Settings::default());
    // Keys in the hand: under a pointer the row keeps its size and the hint one line.
    f.l.input_source = crate::input_source::InputSource::Keyboard;
    f.size = vec2(1280.0, 800.0);
    f.settle();
    let size_hint = "The picture size the PC sends. Higher is sharper and needs more network and decoding power; lower it if the stream stutters.";
    assert!(shows(&f, size_hint), "Resolution has focus on arrival: {:?}", f.drawn);
    assert!(!hint_elided(&f, size_hint));
    let hint = f.placed.iter().find(|(t, ..)| t == size_hint).unwrap().1;
    let label = f.placed.iter().find(|(t, ..)| t == "Resolution").unwrap().1;
    assert!(hint.top() >= label.bottom() - 0.5, "under the label: {hint:?} {label:?}");
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
    // Above the sections of detail, under More settings.
    assert_eq!(rail_entries(&f)[4..7], ["More settings", "Profiles", "Video"], "{:?}", f.drawn);
    let commands = click_text(&mut f, "Profiles");
    assert_eq!(f.l.screen, Screen::Profiles);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "{} commands", commands.len());
    // A narrow window has its chip first.
    f.l.go(Screen::Settings);
    f.size = vec2(800.0, 4000.0);
    f.settle();
    assert_eq!(rail_entries(&f), RAIL_ALL, "{:?}", f.drawn);
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
    for row in ["Resolution", "Automatic bitrate", "Lower bitrate"] {
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

/// Main settings like `settings_with`, with the AccessKit tree on.
fn settings_read_with(changed: Settings) -> Frames {
    let mut f = settings_with(changed);
    f.ctx.enable_accesskit();
    f.settle();
    f
}

/// These PCs, each paired or not and answering or not, every one with both network tests.
fn network_test_pcs(pcs: &[(&str, bool, Reach)]) -> Hosts {
    let answer = nightjar_policy::http::ServerInfo::parse("<root status_code=\"200\"><uniqueid>uid</uniqueid><state>SUNSHINE_SERVER_FREE</state>\
        <PairStatus>1</PairStatus><RustHostVersion>3.0.0</RustHostVersion><MaxVideoPacket>8192</MaxVideoPacket><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes>\
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

/// The Graphics card row is drawn only on a Windows PC with two GPUs.
fn graphics_card_here() -> bool {
    #[cfg(windows)]
    {
        (crate::platform_hooks::hooks().gpu_pair)().is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[path = "settings_tests/overrides_and_vrr_tests.rs"]
mod overrides_and_vrr_tests;
#[path = "settings_tests/network_and_layout_tests.rs"]
mod network_and_layout_tests;
#[path = "settings_tests/presets_and_display_tests.rs"]
mod presets_and_display_tests;
#[path = "settings_tests/simple_and_debug_tests.rs"]
mod simple_and_debug_tests;
